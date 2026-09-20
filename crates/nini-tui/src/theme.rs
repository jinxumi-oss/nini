//! Placeholder theme stub.

#[derive(Default, Clone)]
pub struct Theme;

pub const COLOR_NAMES: &[&str] = &[
    "background", "foreground", "dim", "accent",
    "success", "warning", "error", "info",
];

impl Theme {
    pub fn fg_style(&self, _name: &str) -> ratatui::style::Style {
        ratatui::style::Style::default()
    }
    pub fn bg_style(&self, _name: &str) -> ratatui::style::Style {
        ratatui::style::Style::default()
    }
}

impl Theme {
    pub fn dark() -> Self { Self::default() }
    pub fn light() -> Self { Self::default() }
}
