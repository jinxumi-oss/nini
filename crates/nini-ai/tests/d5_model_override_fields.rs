//! D5: `ModelOverride` 承载 Pi 的 `ModelOverrideSchema` 三个字段。
//!
//! # 这份文件防的是什么
//!
//! `nini_ai::provider_overrides::ModelOverride` 是 provider 构造路径上的
//! **承载类型**：请求时按 `req.model` 查表，模型自己的头、base URL、思考档位
//! 映射、采样参数、方言开关都在这张表里。
//!
//! D5 之前，这张表只有两个格子（`headers` / `base_url`）。Pi 的
//! `ModelOverrideSchema`（`model-config.js:154-171`）还有三个格子：
//! `thinkingLevelMap`（`:157`）、`samplingParams`（`:168`）、`compat`
//! （`:170`）——它们被解析层读进来了，却**无处可放**，也就是无处可查、无处可
//! 用。
//!
//! # 断言的是什么
//!
//! 1. 三个新字段真的被**持有**（构造进去，读出来是同一个值）；
//! 2. `Default` 的三个新字段是 `None`（没写就是没写，不是一个空对象）；
//! 3. 扩展之后，**既有**的 `base_url_for` / `model_headers_for` 行为不变
//!    （新增字段没有把旧行为顶掉）。

use std::collections::BTreeMap;

use nini_ai::provider_overrides::{HeaderOverrides, ModelOverride};
use nini_core::settings::{ProviderCompat, ThinkingLevelMap};

/// 一个思考档位映射。
fn a_thinking_level_map() -> ThinkingLevelMap {
    ThinkingLevelMap {
        off: None,
        minimal: None,
        low: Some("low-effort".to_string()),
        medium: Some("mid-effort".to_string()),
        high: Some("high-effort".to_string()),
        xhigh: None,
        max: None,
    }
}

/// 一个方言开关。
fn a_compat() -> ProviderCompat {
    ProviderCompat {
        max_tokens_field: Some("max_tokens".to_string()),
        supports_store: Some(false),
    }
}

/// 三个新字段被**持有**：构造进去，读出来是同一个值。
#[test]
fn the_new_fields_are_carried() {
    let thinking = a_thinking_level_map();
    let compat = a_compat();
    let mut sampling: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    sampling.insert("top_p".to_string(), serde_json::json!(0.8));
    sampling.insert("presence_penalty".to_string(), serde_json::json!(0.1));

    let entry = ModelOverride {
        headers: vec![("x-model".to_string(), "yes".to_string())],
        base_url: Some("http://model.invalid/v1".to_string()),
        thinking_level_map: Some(thinking.clone()),
        sampling_params: Some(sampling.clone()),
        compat: Some(compat.clone()),
    };

    let map = entry
        .thinking_level_map
        .as_ref()
        .expect("the map is carried");
    assert_eq!(map.low.as_deref(), Some("low-effort"));
    assert_eq!(map.high.as_deref(), Some("high-effort"));
    assert_eq!(map.max, None);

    let params = entry
        .sampling_params
        .as_ref()
        .expect("the parameters are carried");
    assert_eq!(params.get("top_p"), Some(&serde_json::json!(0.8)));
    assert_eq!(
        params.get("presence_penalty"),
        Some(&serde_json::json!(0.1))
    );

    let compat = entry.compat.as_ref().expect("the switches are carried");
    assert_eq!(compat.max_tokens_field.as_deref(), Some("max_tokens"));
    assert_eq!(compat.supports_store, Some(false));
}

/// `Default` 的三个新字段是 `None`：没写就是没写。
///
/// 一个空对象（`Some(默认值)`）会让「这个模型没声明思考档位」和「这个模型
/// 声明了空的思考档位」变成同一件事——而 Pi 区分这两者。
#[test]
fn the_new_fields_default_to_none() {
    let entry = ModelOverride::default();
    assert!(
        entry.thinking_level_map.is_none(),
        "a model that says nothing about thinking levels must not carry a map"
    );
    assert!(
        entry.sampling_params.is_none(),
        "a model that says nothing about sampling must not carry parameters"
    );
    assert!(
        entry.compat.is_none(),
        "a model that says nothing about the dialect must not carry switches"
    );
}

/// 扩展之后，**既有**的查表行为不变。
///
/// `base_url_for` 与 `model_headers_for` 是请求路径上真正被用的两个读口
/// （`nini_ai::openai` / `anthropic` / `openai_responses` 都调它们），新增的
/// 三个字段不能把它们顶掉。
#[test]
fn the_existing_lookup_behaviour_still_holds() {
    let mut model_overrides: BTreeMap<String, ModelOverride> = BTreeMap::new();
    model_overrides.insert(
        "special-model".to_string(),
        ModelOverride {
            headers: vec![("x-model".to_string(), "yes".to_string())],
            base_url: Some("http://model.invalid/v1".to_string()),
            thinking_level_map: Some(a_thinking_level_map()),
            sampling_params: Some(BTreeMap::from([(
                "top_p".to_string(),
                serde_json::json!(0.8),
            )])),
            compat: Some(a_compat()),
        },
    );
    let overrides = HeaderOverrides {
        headers: vec![("x-provider".to_string(), "yes".to_string())],
        auth_header: None,
        model_overrides,
    };

    assert_eq!(
        overrides.base_url_for("special-model"),
        Some("http://model.invalid/v1"),
        "the model-level base URL must still be found"
    );
    assert_eq!(
        overrides.base_url_for("other-model"),
        None,
        "a model without its own base URL still has none"
    );
    assert_eq!(
        overrides.model_headers_for("special-model"),
        &[("x-model".to_string(), "yes".to_string())][..],
        "the model-level headers must still be found"
    );
    assert!(
        overrides.model_headers_for("other-model").is_empty(),
        "a model without its own headers still has none"
    );

    // 合并规则（provider 默认 → provider 级 → model 级）也没变。
    let merged = overrides.resolve(
        "special-model",
        &[("x-default".to_string(), "d".to_string())],
    );
    assert_eq!(
        merged,
        vec![
            ("x-default".to_string(), "d".to_string()),
            ("x-provider".to_string(), "yes".to_string()),
            ("x-model".to_string(), "yes".to_string()),
        ],
        "the header precedence must be unchanged by the new fields"
    );
}
