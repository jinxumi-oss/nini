//! Oracle: nini reads the models a user's `models.json` declares.
//!
//! # The gap
//!
//! Pi's `models.json` is written in Pi's own spelling — `contextWindow`,
//! `maxTokens`, `reasoning` — because that is the shape of Pi's
//! `ModelMetadataSchema`. nini's `ModelConfig` had none of those names:
//! it had `context_window` and nothing else, and serde ignored the keys it
//! did not know.
//!
//! The symptom was silent and total: a user's `models.json` was parsed
//! into an **empty** model list. Every model the user had declared was
//! invisible to nini, and the context window nini used was the 200 000
//! fallback rather than the number written in the file.
//!
//! # What is asserted here
//!
//! 1. Pi's spelling is read (the alias exists at all);
//! 2. a model the shipped catalog does not have becomes a
//!    [`ModelSpec`] — with the user's window, and with **no invented
//!    price**;
//! 3. a model the catalog *does* have is still only overridden, never
//!    duplicated.

use nini_core::model_runtime::{user_model_specs, ModelRuntime};
use nini_core::settings::{load_models_json, ModelsJson};
use std::io::Write as _;

/// A `models.json` in Pi's spelling, with a gateway provider.
fn pi_style_models_json() -> &'static str {
    r#"{
  "providers": {
    "aiio-failover": {
      "baseUrl": "http://127.0.0.1:9877/v1",
      "api": "openai-completions",
      "apiKey": "proxy-handles-auth",
      "models": [
        {
          "id": "MiniMax-M2.7-highspeed",
          "reasoning": true,
          "contextWindow": 204800,
          "maxTokens": 49152
        }
      ]
    }
  }
}"#
}

#[test]
fn pis_spelling_is_read() {
    let cfg: ModelsJson = serde_json::from_str(pi_style_models_json()).expect("models.json parses");
    let provider = cfg.providers.get("aiio-failover").expect("provider");
    let m = provider.models.first().expect("one model");
    assert_eq!(m.id.as_deref(), Some("MiniMax-M2.7-highspeed"));
    assert_eq!(
        m.context_window,
        Some(204_800),
        "Pi writes `contextWindow`; nini must read it, not ignore it"
    );
    assert_eq!(m.max_tokens, Some(49_152));
    assert_eq!(m.reasoning, Some(true));
}

#[test]
fn a_declared_model_becomes_a_spec_with_the_users_window() {
    let cfg: ModelsJson = serde_json::from_str(pi_style_models_json()).expect("models.json parses");
    let specs = user_model_specs(&cfg);
    assert_eq!(specs.len(), 1, "one declared model: {specs:?}");
    let spec = &specs[0];
    assert_eq!(spec.id, "aiio-failover/MiniMax-M2.7-highspeed");
    assert_eq!(spec.provider, "aiio-failover");
    assert_eq!(spec.context_window, 204_800);
    assert_eq!(spec.max_tokens, 49_152);
    assert!(spec.reasoning);
    // No price was stated, so none is invented. A zero cost reads as
    // "unknown" in the UI (the cost pill is hidden), never as "free".
    assert_eq!(spec.cost.input, 0.0);
    assert_eq!(spec.cost.output, 0.0);
}

#[test]
fn a_catalog_model_is_overridden_not_duplicated() {
    let mut cfg: ModelsJson = serde_json::from_str(pi_style_models_json()).expect("parses");
    // `openai/gpt-5` IS in the shipped catalog.
    let provider = cfg.providers.get_mut("aiio-failover").expect("provider");
    provider.models.push(nini_core::settings::ModelConfig {
        id: Some("openai/gpt-5".to_string()),
        context_window: Some(111_111),
        max_tokens: None,
        reasoning: None,
        input: None,
    });

    // It is not a *user* model (the catalog knows it), so it is not listed
    // as one …
    let specs = user_model_specs(&cfg);
    assert!(
        !specs.iter().any(|s| s.id == "openai/gpt-5"),
        "a catalog model must not be re-declared as a user model: {specs:?}"
    );

    // … and the override still applies to the catalog entry.
    let mut runtime = ModelRuntime::with_defaults();
    nini_core::model_runtime::merge_user_overrides(&mut runtime, &cfg);
    let gpt5 = runtime.find_by_id("openai/gpt-5").expect("catalog entry");
    assert_eq!(gpt5.context_window, 111_111);
}

#[test]
fn a_models_json_on_disk_is_read_from_the_agent_directory() {
    // The file the user actually has: the agent directory, not a
    // hand-built `ModelsJson`.
    let guard = nini_core::paths::agent_lock();
    let mut f =
        std::fs::File::create(guard.path().join("models.json")).expect("create models.json");
    writeln!(f, "{}", pi_style_models_json()).expect("write");

    let cfg = load_models_json(std::path::Path::new("/tmp"));
    let specs = user_model_specs(&cfg);
    assert!(
        specs
            .iter()
            .any(|s| s.id == "aiio-failover/MiniMax-M2.7-highspeed"),
        "the model the user declared must be visible to nini: {specs:?}"
    );
}
