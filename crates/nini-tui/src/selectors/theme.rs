//! ThemeSelector — 选主题（dark / light）。
//!
//! 结构照抄 [`crate::selectors::model::ModelSelector`]：`items` /
//! `selected` / `result`，实现 [`SelectorState`]。
//!
//! # 与 model.rs 的一处不同：主题要写盘
//!
//! 主题是"改了必须立刻看见"的设置，所以两件事都要做：
//!
//! 1. **写盘**（[`ThemeSelector::persist`]）：`settings.json` 的 `theme` 键。
//! 2. **立刻生效**（runtime 做）：runtime 读的是
//!    `model_state.settings_snapshot.theme`，写完要把快照里的 theme 换掉。
//!
//! 写盘用 `SettingsManager` 的 merge 式写：先把用户现有的文件读进来，
//! 只改 `theme` 这一个键，再写回去 —— 用户文件里的其它键（`retry` /
//! `max_iterations` / …）不能因为换了个主题就没了。

use std::any::Any;

use crate::selector::{SelectorItem, SelectorOutcome, SelectorState};
use crate::settings::SettingsManager;

/// 内置主题名。
///
/// [`crate::theme::Theme::resolve`] 只认这两个（空串当 dark）；别的名字
/// 指的是 `<agent 目录>/themes/<name>.json` 里的用户主题文件 —— 那种主题
/// 不在这个列表里，但**当前生效的那个**会被补一行（见 [`ThemeSelector::new`]），
/// 否则用户看到的列表里根本没有"我现在用的那个"。
pub const BUILTIN_THEMES: &[&str] = &["dark", "light"];

/// 选主题。
#[derive(Debug, Clone)]
pub struct ThemeSelector {
    /// 用户选中的主题名。
    pub result: Option<String>,
    items: Vec<SelectorItem>,
    selected: usize,
}

impl ThemeSelector {
    /// `current` 是当前生效的主题名（通常是 settings 里的 `theme`）。
    pub fn new(current: Option<&str>) -> Self {
        let current_id = current.unwrap_or("dark").trim().to_string();
        let mut names: Vec<String> = BUILTIN_THEMES.iter().map(|s| s.to_string()).collect();
        // 当前生效的主题若是用户自己的主题文件（themes/<name>.json），
        // 也列一行 —— 列表里没有它，用户会以为自己的主题没了。
        if !current_id.is_empty() && !names.contains(&current_id) {
            names.push(current_id.clone());
        }
        let items: Vec<SelectorItem> = names
            .iter()
            .map(|name| SelectorItem {
                value: name.clone(),
                label: name.clone(),
                description: Some(theme_description(name)),
                is_current: *name == current_id,
            })
            .collect();
        // 打开就停在当前主题那一行。
        let selected = items.iter().position(|i| i.is_current).unwrap_or(0);
        Self {
            result: None,
            items,
            selected,
        }
    }

    /// 把选中的主题写进 `settings.json` 的 `theme` 键。
    ///
    /// merge 式写：先把用户现有的文件读进来，只改 `theme` 这一个键，再写
    /// 回去 —— 用户文件里的其它键不能因为换了个主题就没了。
    ///
    /// 写盘的目标是 agent 目录里那份 `settings.json`（`SettingsManager`
    /// 读写的就是它）；项目级 `.nini/settings.json` 不在这里 —— 那是
    /// nini-core 读的，TUI 不写。
    ///
    /// 返回写盘失败的原因（成功或根本没选中主题时是 None）。
    pub fn persist(&self) -> Option<String> {
        let name = self.result.as_deref()?;
        let path = SettingsManager::default().settings_path();
        if path.as_os_str().is_empty() {
            return Some("没有 settings 文件，主题只在本次会话生效".to_string());
        }
        let mut m = SettingsManager::load_from_disk(path.clone());
        m.set_theme(name);
        match m.flush() {
            Ok(()) => None,
            Err(e) => Some(format!("主题没有写进 {}：{e}", path.display())),
        }
    }
}

/// 一行的说明文字。
fn theme_description(name: &str) -> String {
    match name {
        "dark" => "深色主题（默认）".to_string(),
        "light" => "浅色主题".to_string(),
        other => format!("用户主题：{other}.json"),
    }
}

impl SelectorState for ThemeSelector {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn state_items(&self) -> Vec<SelectorItem> {
        self.items.clone()
    }
    fn state_title(&self) -> String {
        "Theme".to_string()
    }
    fn state_selected(&self) -> usize {
        self.selected
    }
    fn state_set_selected(&mut self, idx: usize) {
        if idx < self.items.len() {
            self.selected = idx;
        }
    }
    fn state_on_select(&mut self) -> SelectorOutcome {
        let item = self.items.get(self.selected).cloned();
        match item {
            Some(it) => {
                self.result = Some(it.value.clone());
                SelectorOutcome::Picked(it)
            }
            None => SelectorOutcome::Cancelled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 内置的两个主题都在列表里。
    #[test]
    fn the_builtin_themes_are_offered() {
        let sel = ThemeSelector::new(Some("dark"));
        let items = sel.state_items();
        let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
        assert!(values.contains(&"dark"), "列表里没有 dark：{values:?}");
        assert!(values.contains(&"light"), "列表里没有 light：{values:?}");
    }

    /// 当前主题被标出来，打开就停在那儿。
    #[test]
    fn the_current_theme_is_marked_and_selected() {
        let sel = ThemeSelector::new(Some("light"));
        let items = sel.state_items();
        let current = items
            .iter()
            .find(|i| i.is_current)
            .expect("没有标出当前主题");
        assert_eq!(current.value, "light");
        assert_eq!(sel.state_selected(), 1, "光标没有停在当前主题上");
    }

    /// 选中一项 → `result` 是它的名字。
    #[test]
    fn picking_a_theme_records_its_name() {
        let mut sel = ThemeSelector::new(Some("dark"));
        assert_eq!(sel.result, None, "还没选就有结果了");
        sel.state_set_selected(1); // light
        let outcome = sel.state_on_select();
        assert!(matches!(outcome, SelectorOutcome::Picked(_)));
        assert_eq!(sel.result.as_deref(), Some("light"));
    }

    /// 当前生效的主题是用户自己的主题文件时（`themes/<name>.json`），
    /// 它也得在列表里 —— 否则用户看到的列表里没有"我现在用的那个"。
    #[test]
    fn a_custom_theme_is_listed_too() {
        let sel = ThemeSelector::new(Some("solarized"));
        let items = sel.state_items();
        let current = items
            .iter()
            .find(|i| i.is_current)
            .expect("没有标出当前主题");
        assert_eq!(current.value, "solarized");
    }
}
