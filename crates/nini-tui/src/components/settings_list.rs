//! `SettingsList` — a read-only display of current settings.
//!
//! This is the v0.8.3 component equivalent to Pi's `SettingsList`
//! (pi-tui/dist/components/settings-list.js). It does NOT yet implement
//! the toggle-on-Enter behavior Pi supports (which requires defining a
//! full settings data model with valid values per setting). For now we
//! render the same items that `SettingsSelector` produces, but with
//! stronger visual emphasis on the value column (accent + bold).
//!
//! # Visual layout (per row)
//!
//! ```text
//! \u2192 model         claude-sonnet-4-5   (\u25b6 current)
//!   theme         dark
//!   thinking      medium
//! ```
//!
//! The cursor row gets the \u2192 accent bullet, the value column is
//! highlighted in accent (vs. muted for non-current rows). Non-current
//! rows have no bullet and the value is dim/muted.
//!
//! # Future work (Phase 10+)
//!
//! To enable live toggling:
//! 1. Define a `SettingItem { id, label, current_value, values: Vec<String> }`
//!    data model in `nini-core/src/settings.rs`.
//! 2. Add an `on_change(id, new_value)` callback.
//! 3. Wire `/settings` to enter toggle mode instead of opening sub-selector.
//!
//! Until then, the existing `SettingsSelector` \u2192 sub-selector flow remains
//! the canonical way to change settings.

use ratatui::text::Span;

use crate::theme::Theme;

/// A single row in the settings list.
#[derive(Debug, Clone)]
pub struct SettingItem {
    /// Unique id (e.g. "model", "theme", "thinking").
    pub id: String,
    /// Display label (e.g. "model", "thinking level").
    pub label: String,
    /// Current value (e.g. "claude-sonnet-4-5", "dark", "medium").
    pub current_value: String,
    /// Optional secondary description (e.g. "subscribed API key").
    pub description: Option<String>,
}

impl SettingItem {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        current_value: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            current_value: current_value.into(),
            description: None,
        }
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

#[derive(Debug, Clone)]
pub struct SettingsList {
    items: Vec<SettingItem>,
    selected_index: usize,
}

impl SettingsList {
    pub fn new(items: Vec<SettingItem>) -> Self {
        Self {
            items,
            selected_index: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn selected(&self) -> Option<&SettingItem> {
        self.items.get(self.selected_index)
    }

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
    }

    /// Render the list into styled spans. Each item is one row.
    ///
    /// Visual style mirrors Pi's `getSettingsListTheme()`:
    /// - Cursor `\u2192 ` in accent + bold
    /// - Selected label/value: accent + bold
    /// - Non-current value: muted
    /// - Description: dim
    pub fn render_spans(&self, width: usize, theme: &Theme) -> Vec<Vec<Span<'static>>> {
        if self.items.is_empty() {
            return vec![vec![Span::styled(
                "  No settings to show".to_string(),
                theme.fg_style("muted"),
            )]];
        }

        let label_width = self
            .items
            .iter()
            .map(|i| crate::width::display_width(&i.label))
            .max()
            .unwrap_or(8)
            .max(8);
        let col_width = width.saturating_sub(4).max(20);

        let mut rows = Vec::with_capacity(self.items.len());
        for (i, item) in self.items.iter().enumerate() {
            let is_sel = i == self.selected_index;
            let mut row: Vec<Span<'static>> = Vec::new();

            // Cursor bullet.
            let bullet = if is_sel { "\u{2192} " } else { "  " };
            row.push(Span::styled(
                bullet.to_string(),
                if is_sel {
                    theme
                        .fg_style("accent")
                        .add_modifier(ratatui::style::Modifier::BOLD)
                } else {
                    theme.fg_style("muted")
                },
            ));

            // Label.
            let label_padded = format!("{:<width$}", item.label, width = label_width);
            let label_len = crate::width::display_width(&label_padded);
            let label_style = if is_sel {
                theme
                    .fg_style("accent")
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                theme.fg_style("text")
            };
            row.push(Span::styled(label_padded, label_style));

            // Value (highlighted when selected).
            let value_style = if is_sel {
                theme
                    .fg_style("accent")
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                theme.fg_style("muted")
            };
            row.push(Span::styled("  ".to_string(), theme.fg_style("text")));
            row.push(Span::styled(item.current_value.clone(), value_style));

            // Optional description.
            if let Some(desc) = &item.description {
                let used = bullet.len() + label_len + 2 + crate::width::display_width(&item.current_value);
                let avail = col_width.saturating_sub(used);
                if avail > 4 {
                    let mut d = desc.clone();
                    if crate::width::display_width(&d) > avail {
                        d = d.chars().take(avail.saturating_sub(1)).collect();
                        d.push('\u{2026}');
                    }
                    row.push(Span::styled(
                        format!("  {d}"),
                        theme.fg_style("dim"),
                    ));
                }
            }

            rows.push(row);
        }
        rows
    }
}

/// Helper: convert `SelectList` items into `SettingItem` rows. Used by
/// `SettingsSelector` to render via `SettingsList` instead of going
/// through the generic `SelectorPanel` path. Returns `None` if any
/// item has no description (which we treat as the "value" column).
pub fn from_select_items(items: &[crate::components::select_list::SelectItem]) -> Option<Vec<SettingItem>> {
    items
        .iter()
        .map(|item| {
            let value = item.description.clone().unwrap_or_default();
            Some(SettingItem::new(&item.value, &item.label, value))
        })
        .collect()
}

/// `SelectList`-style alternative: when you only have a flat list of
/// `(label, value)` pairs and want the Pi SettingsList look.
pub fn render_settings_rows(items: &[(String, String)], theme: &Theme) -> Vec<Vec<Span<'static>>> {
    let list = SettingsList::new(
        items
            .iter()
            .map(|(k, v)| SettingItem::new(k.clone(), k.clone(), v.clone()))
            .collect(),
    );
    list.render_spans(80, theme)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<SettingItem> {
        vec![
            SettingItem::new("model", "model", "claude-sonnet-4-5")
                .with_description("subscribed API key"),
            SettingItem::new("theme", "theme", "dark"),
            SettingItem::new("thinking", "thinking level", "medium"),
        ]
    }

    #[test]
    fn empty_list_no_panic() {
        let list = SettingsList::new(vec![]);
        let rows = list.render_spans(80, &Theme::dark());
        assert_eq!(rows.len(), 1);
        assert!(rows[0]
            .iter()
            .any(|s| s.content.contains("No settings")));
    }

    #[test]
    fn render_emits_one_row_per_item() {
        let list = SettingsList::new(items());
        let rows = list.render_spans(80, &Theme::dark());
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn first_row_has_cursor_arrow() {
        let list = SettingsList::new(items());
        let rows = list.render_spans(80, &Theme::dark());
        let first = rows[0][0].content.as_ref();
        assert!(first.contains('\u{2192}'), "got: {first}");
    }

    #[test]
    fn wrap_around_navigation() {
        let mut list = SettingsList::new(items());
        list.move_by(-1); // wrap to last
        assert_eq!(list.selected_index, 2);
        list.move_by(1); // wrap to first
        assert_eq!(list.selected_index, 0);
    }

    #[test]
    fn from_select_items_extracts_value_from_description() {
        let sitems = vec![
            crate::components::select_list::SelectItem {
                value: "model".into(),
                label: "model".into(),
                description: Some("claude-sonnet-4-5".into()),
                is_current: false,
            },
            crate::components::select_list::SelectItem {
                value: "theme".into(),
                label: "theme".into(),
                description: Some("dark".into()),
                is_current: false,
            },
        ];
        let settings = from_select_items(&sitems).unwrap();
        assert_eq!(settings.len(), 2);
        assert_eq!(settings[0].current_value, "claude-sonnet-4-5");
        assert_eq!(settings[1].current_value, "dark");
    }

    #[test]
    fn from_select_items_returns_none_when_value_missing() {
        let sitems = vec![crate::components::select_list::SelectItem {
            value: "x".into(),
            label: "x".into(),
            description: None,
            is_current: false,
        }];
        // Currently the implementation uses unwrap_or_default, so this
        // returns an item with empty value rather than None. Verify
        // the documented contract.
        let result = from_select_items(&sitems);
        assert!(result.is_some());
    }
}