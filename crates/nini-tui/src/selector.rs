//! Selector abstraction — full-screen overlay panels that let the user
//! pick one item from a list with fuzzy-filtered text input.
//!
//! Each concrete selector (model/session/thinking/trust/settings/tree)
//! implements the [`SelectorState`] trait, providing:
//!
//! - [`SelectorState::state_items`] — full list of selectable items,
//!   re-evaluated after every change so `result` and selection stay fresh
//! - [`SelectorState::state_title`] — title shown at the top of the panel
//! - [`SelectorState::state_selected`] / [`SelectorState::state_set_selected`]
//!   — 0-indexed cursor in the *full* list (visible filtering happens in
//!   the runtime via [`fuzzy_filter`])
//! - [`SelectorState::state_on_select`] — user pressed Enter; the selector
//!   decides whether to store the picked value in its `result` field
//!   and return `Picked`, or fall through with `Back` / `Cancelled`.
//!
//! The runtime layer (see `runtime::handle_selector_key`) handles arrow
//! keys, backspace, escape, and printable input. Selectors don't need to
//! know about input handling.

use std::any::Any;

use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::components::select_list::SelectItem;
use crate::theme::Theme;

/// Alias kept for back-compat with existing callers (selectors/*.rs,
/// runtime.rs, command_palette.rs). New code should use
/// `SelectItem` from `crate::components::select_list` directly.
pub type SelectorItem = SelectItem;

/// Outcome of a user pressing Enter on a selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorOutcome {
    /// The user picked an item. Its id is in the variant.
    Picked(SelectorItem),
    /// The user cancelled (Escape). No state change.
    Cancelled,
    /// The user requested going back to the previous screen.
    Back,
}

pub struct SelectorPanel {
    pub title: String,
    pub query: String,
    pub items: Vec<SelectorItem>,
    pub visible: Vec<usize>,
    pub selected: usize,
    pub theme: Theme,
}

impl SelectorPanel {
    pub fn new(
        title: &str,
        query: &str,
        items: &[SelectorItem],
        visible: &[usize],
        selected: usize,
        theme: &Theme,
    ) -> Self {
        Self {
            title: title.to_string(),
            query: query.to_string(),
            items: items.to_vec(),
            visible: visible.to_vec(),
            selected,
            theme: theme.clone(),
        }
    }
}

impl Widget for SelectorPanel {
    fn render(self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        use ratatui::layout::{Constraint, Direction, Layout};
        use ratatui::style::Modifier;
        use ratatui::text::Line as RLine;

        if area.height < 4 || area.width < 10 {
            return;
        }

        // Outer border around the panel. Fill the inner area with the
        // theme's background so prior transcript text doesn't bleed
        // through (v0.5's selector was effectively transparent).
        //
        // v0.8.3: Fix typo "tool_pending_bg" → "toolPendingBg" so the
        // background actually uses Pi's pending tint (was falling back
        // to DarkGray because the old slot name was never declared).
        let bg_style = self.theme.bg_style("toolPendingBg");
        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .border_style(self.theme.fg_style("borderMuted"))
            .style(bg_style)
            .title(Span::styled(
                format!(" {} ", self.title),
                self.theme.fg_style("accent").add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        block.render(area, buf);

        // Explicit Clear over the inner area: ensures the styled background
        // is applied to every cell, preventing prior transcript content
        // from bleeding through (visible in v0.5 as text fragments
        // peeking out between selector rows).
        let bg = bg_style;
        for y in inner.y..inner.y.saturating_add(inner.height) {
            for x in inner.x..inner.x.saturating_add(inner.width) {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_style(bg);
                }
            }
        }

        if inner.height < 2 {
            return;
        }

        // Split inner into: [input field 1 row] [list n rows].
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(inner);

        // Input prompt.
        let prompt = if self.query.is_empty() {
            Line::from(Span::styled(
                "› type to filter…",
                self.theme.fg_style("dim"),
            ))
        } else {
            Line::from(vec![
                Span::styled("› ", self.theme.fg_style("accent")),
                Span::styled(self.query.clone(), self.theme.fg_style("text")),
                Span::styled("▏", self.theme.fg_style("accent")),
            ])
        };
        prompt.render(rows[0], buf);

        // Visible item rows. v0.8.3: delegate rendering to
        // `SelectList` (Pi parity — cursor arrow, 2-column layout,
        // wrap-around nav all live in one component now).
        let list_area = rows[1];
        if self.visible.is_empty() {
            let msg = if self.items.is_empty() {
                "(no items available)"
            } else {
                "(no matches)"
            };
            Line::from(Span::styled(msg, self.theme.fg_style("dim"))).render(list_area, buf);
            return;
        }

        // Build the SelectList from the visible slice.
        let visible_items: Vec<crate::components::select_list::SelectItem> =
            self.visible.iter().map(|&i| self.items[i].clone()).collect();
        let mut list = crate::components::select_list::SelectList::new(
            visible_items,
            list_area.height as usize,
        );
        // Translate runtime's full-array `selected` to position in
        // the visible slice (SelectList only knows about visible).
        let pos_in_visible = self
            .visible
            .iter()
            .position(|&i| i >= self.selected)
            .unwrap_or_else(|| self.visible.len().saturating_sub(1));
        list.set_selected(pos_in_visible);
        let rendered = list.render_spans(
            list_area.width as usize,
            &self.theme,
            "accent",
            "muted",
            "dim",
            "muted",
        );
        let mut lines: Vec<RLine> = rendered
            .lines
            .into_iter()
            .map(RLine::from)
            .collect();
        // Append (N/M) scroll indicator if items overflow.
        if let Some(scroll_spans) = list.scroll_info_span(&self.theme, "muted") {
            lines.push(RLine::from(scroll_spans));
        }
        ratatui::widgets::Paragraph::new(lines).render(list_area, buf);
    }
}

pub trait SelectorState: Any + Send + std::fmt::Debug {
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn state_items(&self) -> Vec<SelectorItem>;
    fn state_title(&self) -> String;
    fn state_selected(&self) -> usize;
    fn state_set_selected(&mut self, idx: usize);
    fn state_on_select(&mut self) -> SelectorOutcome;
    /// Optional: read the current fuzzy-filter query. Default empty.
    fn state_query(&self) -> String { String::new() }
    /// Optional: set the fuzzy-filter query. Default no-op.
    fn state_set_query(&mut self, _q: String) {}
    /// Back-compat alias used by the runtime layer's downcast helper.
    fn state_as_any_mut(&mut self) -> &mut dyn Any {
        self.as_any_mut()
    }
}

/// Cheap substring match first; fall back to a per-character fuzzy match
/// that lets the user type 'oai' to match 'openai/gpt-5'. Returns indices
/// in the original `items` slice (not the filtered slice).
pub fn fuzzy_filter(query: &str, items: &[SelectorItem]) -> Vec<usize> {
    if query.is_empty() {
        return (0..items.len()).collect();
    }
    let q = query.to_lowercase();
    let mut exact: Vec<usize> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if item.label.to_lowercase().contains(&q)
            || item.value.to_lowercase().contains(&q)
            || item
                .description
                .as_deref()
                .map(|d| d.to_lowercase().contains(&q))
                .unwrap_or(false)
        {
            exact.push(i);
        }
    }
    if !exact.is_empty() {
        return exact;
    }
    let qchars: Vec<char> = q.chars().collect();
    let mut fuzzy: Vec<usize> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let hay = format!(
            "{} {} {}",
            item.label.to_lowercase(),
            item.value.to_lowercase(),
            item.description.as_deref().unwrap_or("")
        );
        // Require all query chars to lie in the SAME whitespace-delimited
        // word of `hay`. This prevents "oai" matching "claude opus" because
        // of the unrelated "anthropic" suffix.
        let mut matched = false;
        for word in hay.split_whitespace() {
            let mut hi = 0usize;
            let mut ok = true;
            for qc in &qchars {
                match word[hi..].find(*qc) {
                    Some(pos) => hi += pos + qc.len_utf8(),
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                matched = true;
                break;
            }
        }
        if matched {
            fuzzy.push(i);
        }
    }
    fuzzy
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, label: &str) -> SelectorItem {
        SelectorItem {
            value: id.to_string(),
            label: label.to_string(),
            description: None,
            is_current: false,
        }
    }

    #[test]
    fn fuzzy_filter_empty_query_returns_all_indices() {
        let items = vec![item("a", "alpha"), item("b", "beta")];
        assert_eq!(fuzzy_filter("", &items), vec![0, 1]);
    }

    #[test]
    fn fuzzy_filter_exact_substring_match() {
        let items = vec![
            item("anthropic/claude", "claude-sonnet-4.5"),
            item("openai/gpt-5", "gpt-5"),
            item("google/gemini", "gemini-2.5-pro"),
        ];
        let v = fuzzy_filter("openai", &items);
        assert_eq!(v, vec![1]);
    }

    #[test]
    fn fuzzy_filter_falls_back_to_subsequence() {
        let items = vec![
            item("anthropic/claude-opus-4.7", "Claude Opus 4.7"),
            item("openai/gpt-5", "GPT-5"),
        ];
        // 'oai' is a subsequence in 'openai/gpt-5' (chars o-p-e-n-a-i/g/p-t-/-5).
        let v = fuzzy_filter("oai", &items);
        assert_eq!(v, vec![1]);
    }

    #[test]
    fn fuzzy_filter_matches_id_field() {
        let items = vec![
            item("claude-opus-4-7", "Opus 4.7"),
            item("gpt-4o", "GPT-4o"),
        ];
        let v = fuzzy_filter("opus", &items);
        assert_eq!(v, vec![0]);
    }

    #[test]
    fn fuzzy_filter_no_match_returns_empty() {
        let items = vec![item("a", "alpha")];
        let v = fuzzy_filter("xyz", &items);
        assert!(v.is_empty());
    }

    #[test]
    fn fuzzy_filter_matches_description() {
        let mut i1 = item("a", "alpha");
        i1.description = Some("Anthropic Claude flagship".into());
        let mut i2 = item("b", "beta");
        i2.description = Some("OpenAI GPT".into());
        let items = vec![i1, i2];
        let v = fuzzy_filter("flagship", &items);
        assert_eq!(v, vec![0]);
    }

    #[test]
    fn panel_widget_renders_border_and_input() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let backend = TestBackend::new(60, 10);
        let mut term = Terminal::new(backend).unwrap();
        let items = vec![
            item("a", "apple"),
            item("b", "banana"),
            item("c", "cherry"),
        ];
        let visible = vec![0usize, 1, 2];
        term.draw(|f| {
            render_selector_panel(
                f,
                "Pick a fruit",
                "",
                &items,
                &visible,
                1,
                &Theme::dark(),
                f.area(),
            )
        })
        .unwrap();
    }

    // helper that delegates to the Widget impl so we don't duplicate logic
    fn render_selector_panel(
        f: &mut ratatui::Frame,
        title: &str,
        query: &str,
        items: &[SelectorItem],
        visible: &[usize],
        selected: usize,
        theme: &Theme,
        area: ratatui::layout::Rect,
    ) {
        use ratatui::widgets::Widget;
        let panel = SelectorPanel::new(title, query, items, visible, selected, theme);
        panel.render(area, f.buffer_mut());
    }
}
