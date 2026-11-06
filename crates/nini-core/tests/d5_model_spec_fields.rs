//! D5: the model-level fields of Pi's `ModelDefinitionSchema` reach the
//! model surface (`ModelSpec`).
//!
//! Pi's `ModelDefinitionSchema` (`model-config.js:139-157`) carries three
//! knobs a *model* may declare on its own:
//!
//!   * `compat?` — the wire-dialect switches of this model;
//!   * `thinkingLevelMap?` — which thinking level maps to which value;
//!   * `samplingParams?` — extra sampling parameters of this model.
//!
//! They were parsed (`settings::ModelConfig` has all three) but dropped on
//! the floor: the model surface had no field to hold them, so a user who
//! declared them saw them nowhere.
//!
//! What is asserted here:
//!
//! 1. the three model-level fields land in the [`ModelSpec`];
//! 2. a model that declares none of them carries `None` for all three
//!    (nothing is invented);
//! 3. `api` follows Pi's priority `definition.api ?? providerConfig.api`
//!    (`provider-composer.js:49`).

use nini_core::model_runtime::user_model_specs;
use nini_core::settings::ModelsJson;

/// A `models.json` with one gateway provider and one model.
fn models_json(model_json: &str) -> ModelsJson {
    let json = format!(
        r#"{{
  "providers": {{
    "aiio-failover": {{
      "baseUrl": "http://127.0.0.1:9877/v1",
      "api": "openai-completions",
      "apiKey": "proxy-handles-auth",
      "models": [ {model_json} ]
    }}
  }}
}}"#
    );
    serde_json::from_str(&json).expect("models.json parses")
}

#[test]
fn the_model_level_fields_reach_the_model_spec() {
    let cfg = models_json(
        r#"{
            "id": "MiniMax-M2.7-highspeed",
            "contextWindow": 204800,
            "maxTokens": 49152,
            "compat": { "maxTokensField": "max_tokens", "supportsStore": false },
            "thinkingLevelMap": { "off": "off", "medium": "medium" },
            "samplingParams": { "top_p": 0.9 }
        }"#,
    );
    let specs = user_model_specs(&cfg);
    assert_eq!(specs.len(), 1, "one declared model: {specs:?}");
    let spec = &specs[0];

    let compat = spec.compat.as_ref().expect("the model's own compat");
    assert_eq!(compat.max_tokens_field.as_deref(), Some("max_tokens"));
    assert_eq!(compat.supports_store, Some(false));

    let map = spec
        .thinking_level_map
        .as_ref()
        .expect("the model's own thinking level map");
    assert_eq!(map.off.as_deref(), Some("off"));
    assert_eq!(map.medium.as_deref(), Some("medium"));

    let params = spec
        .sampling_params
        .as_ref()
        .expect("the model's own sampling params");
    assert_eq!(params.get("top_p"), Some(&serde_json::json!(0.9)));
}

#[test]
fn a_model_that_declares_none_of_them_carries_none() {
    let cfg = models_json(
        r#"{
            "id": "MiniMax-M2.7-highspeed",
            "contextWindow": 204800
        }"#,
    );
    let specs = user_model_specs(&cfg);
    assert_eq!(specs.len(), 1, "one declared model: {specs:?}");
    let spec = &specs[0];
    assert!(spec.compat.is_none(), "nothing is invented");
    assert!(spec.thinking_level_map.is_none(), "nothing is invented");
    assert!(spec.sampling_params.is_none(), "nothing is invented");
}

#[test]
fn the_models_own_api_wins_over_the_providers() {
    let cfg = models_json(
        r#"{
            "id": "MiniMax-M2.7-highspeed",
            "api": "anthropic-messages",
            "contextWindow": 204800
        }"#,
    );
    let specs = user_model_specs(&cfg);
    assert_eq!(specs.len(), 1, "one declared model: {specs:?}");
    assert_eq!(specs[0].api, "anthropic-messages");
}

#[test]
fn without_a_model_api_the_providers_api_is_used() {
    let cfg = models_json(
        r#"{
            "id": "MiniMax-M2.7-highspeed",
            "contextWindow": 204800
        }"#,
    );
    let specs = user_model_specs(&cfg);
    assert_eq!(specs.len(), 1, "one declared model: {specs:?}");
    assert_eq!(specs[0].api, "openai-completions");
}
