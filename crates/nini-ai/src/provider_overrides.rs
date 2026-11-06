//! model 级覆盖。
//!
//! 这一模块承载 **provider 级 / model 级请求头与认证头** 的配置形态，
//! 以及三个 provider（OpenAI Chat Completions、Anthropic Messages、
//! OpenAI Responses）共用的合并规则。
//!
//! 为什么载体在 provider 上而不在 `nini_core::provider::Request` 上：
//! `Request` 有 30 个构造点，没有一个用 `..Default::default()`，其中
//! 20 个在 oracle / 测试文件里。给 `Request` 加字段会打破这 20 个
//! 构造点，而项目的纪律是「oracle 只能引用修复前后都存在的 API」。
//! 所以这些配置挂在 provider 构造上，请求时再按 `req.model` 查表。

use nini_core::settings::{ProviderCompat, ThinkingLevelMap};
use reqwest::RequestBuilder;

/// model 级覆盖。
/// 一个模型可以有自己的请求头和 base URL。
///
/// D5: Pi 的 `ModelOverrideSchema`（`model-config.js:154-171`）还带着三个
/// 开关——思考档位映射、采样参数、方言兼容——它们在这里**承载**：provider
/// 构造路径按 Pi 的口径把它们合并进这张表（合并规则见
/// `nini_cli::provider_factory`），请求时再按 `req.model` 查表。
#[derive(Debug, Clone, Default)]
pub struct ModelOverride {
    /// 这个模型专属的请求头。
    pub headers: Vec<(String, String)>,
    /// 这个模型专属的 base URL（有它就不用 provider 自己的）。
    pub base_url: Option<String>,
    /// Which thinking level maps to which value.
    ///
    /// Pi: `ModelOverrideSchema.thinkingLevelMap?` (`model-config.js:157`)
    /// — the same shape as the model's own `thinkingLevelMap`
    /// (`model-config.js:145`): one optional value per thinking level.
    ///
    /// Pi merges it **per level** with the model's own map
    /// (`applyModelOverride`, `provider-composer.js:27-29`:
    /// `{...model.thinkingLevelMap, ...override.thinkingLevelMap}`) — the
    /// override wins for the levels it names, the model's own values
    /// survive for the levels it does not.
    pub thinking_level_map: Option<ThinkingLevelMap>,
    /// Extra sampling parameters sent with every request of this model.
    ///
    /// Pi: `ModelOverrideSchema.samplingParams?` (`model-config.js:168`)
    /// — `Record<string, unknown>`, merged **per key** with the model's own
    /// params (`provider-composer.js:42-44`:
    /// `{...model.samplingParams, ...override.samplingParams}`).
    pub sampling_params: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    /// The wire-dialect switches of this model.
    ///
    /// Pi: `ModelOverrideSchema.compat?` (`model-config.js:170`) — merged
    /// **per key** with the model's own compat (`mergeCompat`,
    /// `provider-composer.js:5-21`, applied at `provider-composer.js:45`:
    /// `mergeCompat(model.compat, override.compat)`).
    pub compat: Option<ProviderCompat>,
}

impl PartialEq for ModelOverride {
    /// 手写而不是 derive：`serde_json::Value` 只有 `PartialEq`（它里面是
    /// `f64`），而 `nini_core::settings` 的 `ThinkingLevelMap` /
    /// `ProviderCompat` 没有 derive `PartialEq`——那个 crate 是**解析层**，
    /// 这一轮不改它，所以这里逐字段比。
    ///
    /// `Eq` 一并去掉：`Value` 不是 `Eq`，加不回来。
    fn eq(&self, other: &Self) -> bool {
        self.headers == other.headers
            && self.base_url == other.base_url
            && thinking_level_map_eq(&self.thinking_level_map, &other.thinking_level_map)
            && self.sampling_params == other.sampling_params
            && compat_eq(&self.compat, &other.compat)
    }
}

/// 两个 `ThinkingLevelMap` 是不是一样（逐档位比）。
fn thinking_level_map_eq(a: &Option<ThinkingLevelMap>, b: &Option<ThinkingLevelMap>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.off == b.off
                && a.minimal == b.minimal
                && a.low == b.low
                && a.medium == b.medium
                && a.high == b.high
                && a.xhigh == b.xhigh
                && a.max == b.max
        }
        _ => false,
    }
}

/// 两个 `ProviderCompat` 是不是一样（逐开关比）。
fn compat_eq(a: &Option<ProviderCompat>, b: &Option<ProviderCompat>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.max_tokens_field == b.max_tokens_field && a.supports_store == b.supports_store
        }
        _ => false,
    }
}

/// provider 级与 model 级的请求头配置。
///
/// 三个 provider 共用同一份配置与同一套合并规则，因此语义天然一致。
#[derive(Debug, Clone, Default)]
pub struct HeaderOverrides {
    /// provider 级自定义请求头：附加到**每个**请求上。
    pub headers: Vec<(String, String)>,
    /// 认证头的名字。`None` 表示用各 provider 的默认写法
    /// （OpenAI 系是 `Authorization: Bearer <key>`，Anthropic 是
    /// `x-api-key: <key>`）；`Some(name)` 时用这个名字发**裸值**的 key
    /// （不带 `Bearer` 前缀）。
    pub auth_header: Option<&'static str>,
    /// model 级覆盖表，键是模型名（`req.model` 的原样值）。
    pub model_overrides: std::collections::BTreeMap<String, ModelOverride>,
}

impl HeaderOverrides {
    /// 该模型专属的 base URL（`None` 表示用 provider 自己的）。
    pub fn base_url_for(&self, model: &str) -> Option<&str> {
        self.model_overrides
            .get(model)
            .and_then(|o| o.base_url.as_deref())
    }

    /// 该模型专属的请求头。
    pub fn model_headers_for(&self, model: &str) -> &[(String, String)] {
        self.model_overrides
            .get(model)
            .map(|o| o.headers.as_slice())
            .unwrap_or(&[])
    }

    /// 合并出最终要发的请求头。
    ///
    /// 顺序即优先级：**provider 默认头 → provider 级自定义头 →
    /// model 级自定义头**，同名（大小写不敏感）后者覆盖前者。
    pub fn resolve(&self, model: &str, defaults: &[(String, String)]) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for (name, value) in defaults
            .iter()
            .chain(self.headers.iter())
            .chain(self.model_headers_for(model).iter())
        {
            let lower = name.to_ascii_lowercase();
            match out
                .iter_mut()
                .find(|(existing, _)| existing.to_ascii_lowercase() == lower)
            {
                Some(slot) => slot.1 = value.clone(),
                None => out.push((name.clone(), value.clone())),
            }
        }
        out
    }
}

/// 把合并好的请求头写进请求。
pub fn apply_headers(builder: RequestBuilder, headers: &[(String, String)]) -> RequestBuilder {
    headers
        .iter()
        .fold(builder, |acc, (name, value)| acc.header(name, value))
}
