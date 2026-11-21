//! SettingsSelector — pick a setting category (model / theme / thinking /
//! 存活阈值).
//!
//! 选一个分类**会**打开对应的子选择器（model → `ModelSelector`、theme →
//! `ThemeSelector`、thinking → `ThinkingSelector`）：接线在 `runtime.rs`
//! 的 `chained_selector` 里。
//!
//! 这个模块以前自承"选一个分类什么都不会发生"—— 那时候确实什么都不会
//! 发生：既没有 `ThemeSelector`，也没有从分类到子选择器的接线。今天接线
//! 补上了，只读行（存活阈值）没有子选择器可开，改为推一条 notice 告诉
//! 用户去哪儿改 —— 一行死胡同比不说更糟。

use std::any::Any;

use crate::selector::{SelectorItem, SelectorOutcome, SelectorState};
use crate::settings::SettingsManager;

/// 存活阈值这一行的分类名。
///
/// 它是**只读**的：交互式数值编辑不在这一轮里，所以选中它只会得到一条
/// "去 settings.json 改"的提示。
pub const LIVENESS_CATEGORY: &str = "liveness";

/// 存活阈值这一行的说明：当前生效的四个阈值 + 去哪儿改。
///
/// 阈值读的是**进程级生效值**（`nini_core::liveness::liveness_settings()`），
/// 不是 settings.json 里的原始值 —— 0 会被清洗成默认值，进程里生效的
/// 就是清洗后的那份。
fn liveness_description() -> String {
    let lv = nini_core::liveness::liveness_settings();
    format!(
        "provider 响应头 {}s • 流空闲 {}s • 等模型 {}s • 等工具 {}s（改 settings.json 的 liveness 段）",
        lv.provider_response_secs,
        lv.provider_stream_idle_secs,
        lv.no_progress_model_secs,
        lv.no_progress_tool_secs,
    )
}

/// 选中一个**只读**分类时，告诉用户去哪儿改。
///
/// 只读的行（本轮只有"存活阈值"）没有子选择器可开 —— 交互式数值编辑不在
/// 这一轮里。用户必须知道去哪儿改，否则这一行就是个死胡同。
pub fn read_only_notice(category: &str) -> Option<String> {
    match category {
        LIVENESS_CATEGORY => Some(
            "存活阈值改 settings.json 的 liveness 段（provider 响应头 / 流空闲 / 等模型 / 等工具，单位秒）"
                .to_string(),
        ),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct SettingsSelector {
    pub result: Option<String>,
    pub settings: SettingsSnapshot,
    pub items: Vec<SelectorItem>,
    pub selected: usize,
}

/// Snapshot of the current settings values, shown in the description.
#[derive(Debug, Clone, Default)]
pub struct SettingsSnapshot {
    pub model: String,
    pub theme: String,
    pub thinking: String,
}

impl SettingsSnapshot {
    pub fn model_name(&self) -> String {
        self.model.clone()
    }
}

impl SettingsSelector {
    pub fn new(manager: SettingsManager) -> Self {
        let snapshot = SettingsSnapshot {
            model: manager.settings.default_model.clone().unwrap_or_default(),
            theme: manager.theme.clone(),
            thinking: manager
                .settings
                .default_thinking_level
                .clone()
                .unwrap_or_default(),
        };
        Self::with_snapshot(snapshot)
    }

    pub fn with_snapshot(snapshot: SettingsSnapshot) -> Self {
        // An unset value used to render as an empty string, which in a
        // selector list is indistinguishable from a row that failed to
        // load. It says so instead.
        // A nested `fn`, not a closure: a closure would have to infer a
        // single lifetime for both arms, and the return type differs.
        fn show(v: &str) -> &str {
            if v.is_empty() {
                "(unset)"
            } else {
                v
            }
        }
        let items = vec![
            SelectorItem {
                value: "model".to_string(),
                label: "model".to_string(),
                description: Some(show(&snapshot.model).to_string()),
                is_current: false,
            },
            SelectorItem {
                value: "theme".to_string(),
                label: "theme".to_string(),
                description: Some(show(&snapshot.theme).to_string()),
                is_current: false,
            },
            SelectorItem {
                value: "thinking".to_string(),
                label: "thinking".to_string(),
                description: Some(show(&snapshot.thinking).to_string()),
                is_current: false,
            },
            // 只读行：这一轮没有交互式数值编辑，选中它只会得到一条
            // "去 settings.json 改"的提示。放在最后 —— 前面三行的位置
            // 是 `picked_category(0..2)` 认的。
            SelectorItem {
                value: LIVENESS_CATEGORY.to_string(),
                label: "存活阈值".to_string(),
                description: Some(liveness_description()),
                is_current: false,
            },
        ];
        Self {
            result: None,
            settings: snapshot,
            items,
            selected: 0,
        }
    }

    /// Which category the user picked: `"model"`, `"theme"`, `"thinking"`
    /// or `"liveness"`（只读行）。
    ///
    /// This used to be
    ///
    /// ```text
    /// /// Persist the picked setting back to a SettingsManager. Used by
    /// /// runtime when the user picks e.g. "model" (sub-selector opens).
    /// pub fn apply(&mut self, _idx: usize) -> Option<String> {
    ///     self.result.clone()
    /// }
    /// ```
    ///
    /// Three things wrong with it. It persisted nothing and the selector
    /// holds no manager - only a `SettingsSnapshot` - so the comment
    /// described an object that is not there. It ignored its index, so
    /// every category returned the same answer. And its caller treated
    /// that answer as a new model name, so picking "theme" or "thinking"
    /// reported a model change that had not happened.
    ///
    /// The category at `idx`, or `None` if that row does not exist.
    ///
    /// Any in-range index answers. The first version of this had an
    /// `idx == self.selected` guard on it, which meant a caller asking
    /// about a row the user was not currently on got nothing - the same
    /// index-blindness as the function it replaced, just pointing the
    /// other way.
    pub fn picked_category(&self, idx: usize) -> Option<&str> {
        self.items.get(idx).map(|i| i.value.as_str())
    }
}

impl SelectorState for SettingsSelector {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn state_items(&self) -> Vec<SelectorItem> {
        self.items.clone()
    }
    fn state_title(&self) -> String {
        "Settings".to_string()
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

    /// `/settings` 里有一行"存活阈值"，描述里是**当前生效的**四个阈值。
    #[test]
    fn the_liveness_row_shows_the_four_thresholds() {
        let sel = SettingsSelector::with_snapshot(SettingsSnapshot::default());
        let items = sel.state_items();
        let row = items
            .iter()
            .find(|i| i.value == LIVENESS_CATEGORY)
            .expect("列表里没有存活阈值这一行");
        assert_eq!(row.label, "存活阈值");

        let desc = row.description.clone().unwrap_or_default();
        let lv = nini_core::liveness::liveness_settings();
        for (label, secs) in [
            ("provider 响应头", lv.provider_response_secs),
            ("流空闲", lv.provider_stream_idle_secs),
            ("等模型", lv.no_progress_model_secs),
            ("等工具", lv.no_progress_tool_secs),
        ] {
            assert!(
                desc.contains(&format!("{label} {secs}s")),
                "描述里没有 {label} {secs}s：{desc}"
            );
        }
        assert!(
            desc.contains("settings.json"),
            "描述没告诉用户去哪儿改：{desc}"
        );
    }

    /// 只读行没有子选择器可开 —— 至少要有一条 notice 告诉用户去哪儿改。
    #[test]
    fn the_read_only_row_says_where_to_change_it() {
        let notice = read_only_notice(LIVENESS_CATEGORY).expect("存活阈值没有提示");
        assert!(
            notice.contains("liveness"),
            "提示里没说 liveness 段：{notice}"
        );
    }
}
