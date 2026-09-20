//! Selector stub.
use std::any::Any;

#[derive(Default, Clone)]
pub struct SelectorItem {
    #[allow(dead_code)] pub extra: String,
    pub id: String,
    pub label: String,
}
pub struct SelectorPanel {
    pub items: Vec<SelectorItem>,
}
pub enum SelectorOutcome {
    Picked(SelectorItem),
    Cancelled,
    Back,
}
impl SelectorPanel {
    pub fn new(_title: &str, _query: &str, items: &[SelectorItem], _visible: &[usize], _selected: usize, _theme: &crate::theme::Theme) -> Self {
        Self { items: items.to_vec() }
    }
}

pub trait SelectorState: Any + Send + std::fmt::Debug {
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn state_items(&self) -> Vec<SelectorItem> { vec![] }
    fn state_title(&self) -> String { String::new() }
    fn state_selected(&self) -> usize { 0 }
    fn state_set_selected(&mut self, _idx: usize) {}
    fn state_on_select(&mut self) -> crate::selector::SelectorOutcome { crate::selector::SelectorOutcome::Cancelled }
    fn state_as_any_mut(&mut self) -> &mut dyn Any { self.as_any_mut() }
}

/// Fuzzy-filter items by query, returning matching indices.
pub fn fuzzy_filter(query: &str, items: &[SelectorItem]) -> Vec<usize> {
    if query.is_empty() {
        return (0..items.len()).collect();
    }
    let q = query.to_lowercase();
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.label.to_lowercase().contains(&q) || item.id.to_lowercase().contains(&q)
        })
        .map(|(i, _)| i)
        .collect()
}


impl ratatui::widgets::Widget for SelectorPanel {
    fn render(self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        // placeholder: render nothing
        let _ = (area, buf);
    }
}

impl std::fmt::Display for SelectorItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label)
    }
}
impl std::fmt::Debug for SelectorItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SelectorItem({})", self.label)
    }
}
