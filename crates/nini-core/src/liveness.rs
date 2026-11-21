//! 存活指示器的时间阈值与进程级生效值。
//!
//! 为什么是"进程级"：provider 的流空闲超时在 nini-ai 层（那里没有 settings
//! 句柄），TUI 的"无进展"阈值在渲染层。两者要读同一份用户配置，又都不能
//! 各自去读盘 —— 于是配置在进程启动时解析一次，之后各处从同一处读。

use std::sync::{LazyLock, RwLock};

/// bash 工具的默认命令超时（秒）。
///
/// 与 `nini_tools::bash::DEFAULT_TIMEOUT_SECS` 是同一个数：等工具的
/// 无进展阈值比它短，正常的 `cargo test` / `cargo build` 就会被误报成
/// "卡住了"。
const BASH_DEFAULT_TIMEOUT_SECS: u64 = 120;

/// 存活指示器的四个时间阈值（秒）。
///
/// 键名风格与 nini-core 其它设置一致：snake_case 为主，camelCase 作为别名。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct LivenessSettings {
    /// provider 流的空闲超时：两个 chunk 之间超过这么久没有数据，就判定
    /// 上游挂死。
    ///
    /// 等**响应头**另有一个阈值（[`LivenessSettings::provider_response_secs`]）：
    /// 响应头应该很快到，而 chunk 之间的间隔可以长得多。
    #[serde(alias = "providerStreamIdleSecs")]
    pub provider_stream_idle_secs: u64,
    /// 等模型时，多久没有任何事件就算"无进展"。
    #[serde(alias = "noProgressModelSecs")]
    pub no_progress_model_secs: u64,
    /// 等工具时，多久没有任何事件就算"无进展"。
    ///
    /// 必须大于 [`LivenessSettings::provider_stream_idle_secs`]，
    /// 否则正常的长工具会被误报成挂死；并且不能小于 120（bash 的默认
    /// 超时），否则正常的 `cargo test` / `cargo build` 会被误报成卡住。
    #[serde(alias = "noProgressToolSecs")]
    pub no_progress_tool_secs: u64,
    /// 等**响应头**的上限（秒）：请求已发出，服务器却迟迟不回响应头。
    ///
    /// 与 [`LivenessSettings::provider_stream_idle_secs`] 分开：响应头应该在
    /// 很快内到达（服务器收到了请求就回），而流里的 chunk 间隔可以长得多
    /// （模型在思考）。两者共用一个值，会让"首响应慢"的场景要么等太久、
    /// 要么误杀健康的长流。
    #[serde(alias = "providerResponseSecs")]
    pub provider_response_secs: u64,
}

impl Default for LivenessSettings {
    fn default() -> Self {
        Self {
            provider_stream_idle_secs: 120,
            no_progress_model_secs: 60,
            no_progress_tool_secs: 180,
            provider_response_secs: 60,
        }
    }
}

/// 校验阈值配置，返回**人类可读的警告**（空 = 没问题）。
///
/// 只报**真的会咬人**的情况，不做风格建议：
///  * 阈值为 0 → 立即触发（等响应头 0 秒 = 每个请求都判挂死）；
///    这种值**不能生效**，调用方应当回退到默认值。
///  * 等工具的无进展阈值短于 bash 的默认超时（120s）→ 正常的 cargo test /
///    build 会被误报成"卡住了"。
pub fn validate(settings: &LivenessSettings) -> Vec<String> {
    let mut warnings = Vec::new();

    if settings.provider_stream_idle_secs == 0 {
        warnings.push("provider 流空闲超时为 0 秒：每个响应都会立刻被判为挂死".to_string());
    }
    if settings.provider_response_secs == 0 {
        warnings.push("等响应头超时为 0 秒：每个请求都会立刻被判为挂死".to_string());
    }
    if settings.no_progress_model_secs == 0 {
        warnings.push("等模型无进展超时为 0 秒：模型的每次输出都会被立刻判为卡住".to_string());
    }
    if settings.no_progress_tool_secs == 0 {
        warnings.push("等工具无进展超时为 0 秒：工具的每次运行都会被立刻判为卡住".to_string());
    } else if settings.no_progress_tool_secs < BASH_DEFAULT_TIMEOUT_SECS {
        warnings.push(format!(
            "等工具无进展超时为 {} 秒：短于 bash 的默认超时 {} 秒，正常的 cargo test / build 会被误报成卡住了",
            settings.no_progress_tool_secs, BASH_DEFAULT_TIMEOUT_SECS
        ));
    }

    warnings
}

/// 把非法值（0）换成默认值，其余原样保留。
///
/// 非法值**不生效**：调用方（进程启动时）拿本函数的产物存进全局，
/// 而"你写的这个值被换掉了"由 [`validate`] 的警告告诉用户。
pub fn sanitized(settings: LivenessSettings) -> LivenessSettings {
    let d = LivenessSettings::default();
    LivenessSettings {
        provider_stream_idle_secs: non_zero_or(
            settings.provider_stream_idle_secs,
            d.provider_stream_idle_secs,
        ),
        no_progress_model_secs: non_zero_or(
            settings.no_progress_model_secs,
            d.no_progress_model_secs,
        ),
        no_progress_tool_secs: non_zero_or(settings.no_progress_tool_secs, d.no_progress_tool_secs),
        provider_response_secs: non_zero_or(
            settings.provider_response_secs,
            d.provider_response_secs,
        ),
    }
}

/// 0 换成默认值，其余原样。
fn non_zero_or(value: u64, fallback: u64) -> u64 {
    if value == 0 {
        fallback
    } else {
        value
    }
}

/// 进程级生效值。`LazyLock` + `RwLock`：写一次、读无数次（每帧一次）。
static SETTINGS: LazyLock<RwLock<LivenessSettings>> =
    LazyLock::new(|| RwLock::new(LivenessSettings::default()));

/// 进程级的阈值校验警告（`set_liveness_settings` 时算好存下）。
static WARNINGS: LazyLock<RwLock<Vec<String>>> = LazyLock::new(|| RwLock::new(Vec::new()));

/// 设置本次进程生效的存活阈值（进程启动时调用一次）。
///
/// 存进全局的是**清洗后**的值（0 → 默认），而警告是拿**原始值**算的：
/// "你写的 0 被换掉了"这条信息不能因为清洗而丢掉。
pub fn set_liveness_settings(settings: LivenessSettings) {
    let warnings = validate(&settings);
    *SETTINGS.write().unwrap_or_else(|e| e.into_inner()) = sanitized(settings);
    *WARNINGS.write().unwrap_or_else(|e| e.into_inner()) = warnings;
}

/// 读取本次进程生效的存活阈值。
pub fn liveness_settings() -> LivenessSettings {
    *SETTINGS.read().unwrap_or_else(|e| e.into_inner())
}

/// 本次进程的阈值**校验警告**（`set_liveness_settings` 时算好存下）。
/// 空 = 没有问题。
///
/// 返回的是**存下来的**那一份（不是每次重算）—— 因为生效值里的 0 已经被
/// 清洗成默认值，重算会丢掉"你写的 0 被换掉了"这条信息。
pub fn liveness_warnings() -> Vec<String> {
    WARNINGS.read().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 进程级全局状态：测试是并行跑的，两个测试同时改它就会互相看错。
    /// 改全局的测试都先拿这把锁，改完立刻把全局放回默认值。
    static GLOBAL: Mutex<()> = Mutex::new(());

    /// 默认阈值：120 / 60 / 180 / 60。
    ///
    /// 这里断言的是 `Default` 的产物，不是全局里的当前值 —— 全局状态由
    /// 并行运行的其它测试共享，断言"别人没改过它"必然是脆弱的。
    #[test]
    fn the_default_thresholds_are_120_60_180_and_60() {
        let d = LivenessSettings::default();
        assert_eq!(d.provider_stream_idle_secs, 120);
        assert_eq!(d.no_progress_model_secs, 60);
        assert_eq!(d.no_progress_tool_secs, 180);
        assert_eq!(d.provider_response_secs, 60);
    }

    /// `set` 之后 `liveness_settings()` 读回同一个值。
    ///
    /// 改的是进程级全局，所以测完必须改回默认值 —— 否则会污染同 crate 的
    /// 其它测试。
    #[test]
    fn a_set_threshold_is_read_back() {
        let _guard = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        let custom = LivenessSettings {
            provider_stream_idle_secs: 11,
            no_progress_model_secs: 22,
            no_progress_tool_secs: 33,
            provider_response_secs: 44,
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

    /// 全默认 → 没有警告。
    #[test]
    fn the_default_thresholds_have_no_warnings() {
        assert!(
            validate(&LivenessSettings::default()).is_empty(),
            "the default thresholds must not produce a warning"
        );
    }

    /// 阈值为 0 → 警告：这个值会**立刻**触发。
    #[test]
    fn a_zero_threshold_is_reported_as_firing_immediately() {
        let warnings = validate(&LivenessSettings {
            provider_response_secs: 0,
            ..LivenessSettings::default()
        });
        assert!(
            warnings.iter().any(|w| w.contains("0 秒")),
            "a threshold of 0 must be reported: {warnings:?}"
        );
    }

    /// 等工具的无进展阈值短于 bash 的默认超时（120s）→ 警告：正常的
    /// cargo test / build 会被误报成卡住。
    #[test]
    fn a_short_tool_timeout_is_reported_as_a_false_positive() {
        let warnings = validate(&LivenessSettings {
            no_progress_tool_secs: 60,
            ..LivenessSettings::default()
        });
        assert!(
            warnings.iter().any(|w| w.contains("误报")),
            "a tool timeout below the bash default must be reported as a false \
             positive: {warnings:?}"
        );
    }

    /// `sanitized`：0 换成默认值，其余原样保留。
    #[test]
    fn sanitized_replaces_the_zeros_and_keeps_the_rest() {
        let s = sanitized(LivenessSettings {
            provider_stream_idle_secs: 0,
            no_progress_model_secs: 22,
            no_progress_tool_secs: 133,
            provider_response_secs: 0,
        });
        assert_eq!(s.provider_stream_idle_secs, 120);
        assert_eq!(s.no_progress_model_secs, 22);
        assert_eq!(s.no_progress_tool_secs, 133);
        assert_eq!(s.provider_response_secs, 60);
    }

    /// `set` 之后：警告是拿**原始值**算的（"你写的 0 被换掉了"这条信息
    /// 不能因为清洗而丢掉），而生效值里的 0 已经换成了默认值。
    #[test]
    fn a_set_keeps_the_warnings_and_sanitizes_the_thresholds() {
        let _guard = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        set_liveness_settings(LivenessSettings {
            provider_stream_idle_secs: 0,
            no_progress_model_secs: 22,
            no_progress_tool_secs: 133,
            provider_response_secs: 44,
        });

        let warnings = liveness_warnings();
        let effective = liveness_settings();

        // 先把全局改回默认值，再断言：断言失败也不会留下脏状态。
        set_liveness_settings(LivenessSettings::default());

        assert!(
            warnings.iter().any(|w| w.contains("0 秒")),
            "the warning about the threshold that was set to 0 must be kept: \
             {warnings:?}"
        );
        assert_eq!(
            effective.provider_stream_idle_secs, 120,
            "a threshold of 0 must not be in force"
        );
        assert_eq!(effective.no_progress_model_secs, 22);
        assert_eq!(effective.no_progress_tool_secs, 133);
        assert_eq!(effective.provider_response_secs, 44);
    }
}
