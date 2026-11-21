//! 存活指示器的时间阈值与进程级生效值。
//!
//! 为什么是"进程级"：provider 的流空闲超时在 nini-ai 层（那里没有 settings
//! 句柄），TUI 的"无进展"阈值在渲染层。两者要读同一份用户配置，又都不能
//! 各自去读盘 —— 于是配置在进程启动时解析一次，之后各处从同一处读。

use std::sync::{LazyLock, RwLock};

/// 存活指示器的三个时间阈值（秒）。
///
/// 键名风格与 nini-core 其它设置一致：snake_case 为主，camelCase 作为别名。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct LivenessSettings {
    /// provider 流的空闲超时：两个 chunk 之间、或等待响应头，超过这么久
    /// 没有数据就判定上游挂死。
    #[serde(alias = "providerStreamIdleSecs")]
    pub provider_stream_idle_secs: u64,
    /// 等模型时，多久没有任何事件就算"无进展"。
    #[serde(alias = "noProgressModelSecs")]
    pub no_progress_model_secs: u64,
    /// 等工具时，多久没有任何事件就算"无进展"。
    ///
    /// 必须大于 [`LivenessSettings::provider_stream_idle_secs`]，
    /// 否则正常的长工具会被误报成挂死。
    #[serde(alias = "noProgressToolSecs")]
    pub no_progress_tool_secs: u64,
}

impl Default for LivenessSettings {
    fn default() -> Self {
        Self {
            provider_stream_idle_secs: 120,
            no_progress_model_secs: 60,
            no_progress_tool_secs: 180,
        }
    }
}

/// 进程级生效值。`LazyLock` + `RwLock`：写一次、读无数次（每帧一次）。
static SETTINGS: LazyLock<RwLock<LivenessSettings>> =
    LazyLock::new(|| RwLock::new(LivenessSettings::default()));

/// 设置本次进程生效的存活阈值（进程启动时调用一次）。
pub fn set_liveness_settings(settings: LivenessSettings) {
    *SETTINGS.write().unwrap_or_else(|e| e.into_inner()) = settings;
}

/// 读取本次进程生效的存活阈值。
pub fn liveness_settings() -> LivenessSettings {
    *SETTINGS.read().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认阈值：120 / 60 / 180。
    ///
    /// 这里断言的是 `Default` 的产物，不是全局里的当前值 —— 全局状态由
    /// 并行运行的其它测试共享，断言"别人没改过它"必然是脆弱的。
    #[test]
    fn the_default_thresholds_are_120_60_and_180() {
        let d = LivenessSettings::default();
        assert_eq!(d.provider_stream_idle_secs, 120);
        assert_eq!(d.no_progress_model_secs, 60);
        assert_eq!(d.no_progress_tool_secs, 180);
    }

    /// `set` 之后 `liveness_settings()` 读回同一个值。
    ///
    /// 改的是进程级全局，所以测完必须改回默认值 —— 否则会污染同 crate 的
    /// 其它测试。
    #[test]
    fn a_set_threshold_is_read_back() {
        let custom = LivenessSettings {
            provider_stream_idle_secs: 11,
            no_progress_model_secs: 22,
            no_progress_tool_secs: 33,
        };
        set_liveness_settings(custom);
        let read = liveness_settings();
        // 先改回默认值，再断言：断言失败也不会把全局留在脏状态。
        set_liveness_settings(LivenessSettings::default());
        assert_eq!(read, custom, "the value that was set was not read back");
    }

    /// 读出来的是一份**值副本**：改副本不影响读到的原值（`Copy`）。
    ///
    /// 这正是各层可以把阈值拷进自己的状态里、而不用担心被别人的改动影响
    /// 的原因。
    #[test]
    fn the_read_thresholds_are_a_copy() {
        let read = liveness_settings();
        let saved = read.provider_stream_idle_secs;
        let mut copy = read;
        copy.provider_stream_idle_secs = saved.wrapping_add(1);
        // 改了副本，原值不动 —— 各层把阈值拷进自己状态里，靠的就是这一点。
        assert_eq!(
            read.provider_stream_idle_secs, saved,
            "the read thresholds must be a copy, not a shared reference"
        );
        assert_eq!(copy.provider_stream_idle_secs, saved.wrapping_add(1));
    }
}
