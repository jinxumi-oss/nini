//! Oracle: the model catalog carries Pi's real data.
//!
//! Ground truth is Pi 0.84.2's own shipped catalog, not a reading of its
//! source: `pi-ai/dist/providers/data/*.json`, 39 provider files holding
//! 1267 model entries. nini's catalog is generated from those files by
//! `scripts/generate-model-catalog.py`, filtered to the providers nini can
//! actually reach.
//!
//! What this replaces: ten hand-typed ids, every one of them given
//! `context_window: 128_000`, three of them (`cohere/command-r-plus`,
//! `deepseek/deepseek-chat`, `groq/llama-3.3-70b`) naming models Pi does not
//! have, a `from_models_json` that ignored its argument, and a `resolve`
//! that returned `None` unconditionally.
//!
//! The values asserted here are Pi's, quoted from its data.

use nini_core::model_runtime::{resolve, ModelRuntime, ModelSpec};

fn spec(id: &str) -> ModelSpec {
    ModelRuntime::with_defaults()
        .find_by_id(id)
        .unwrap_or_else(|| panic!("{id} must be in the catalog"))
}

#[test]
fn claude_sonnet_4_5_carries_pis_own_numbers() {
    let m = spec("anthropic/claude-sonnet-4-5");
    assert_eq!(m.context_window, 1_000_000, "Pi's data says 1M, not 128k");
    assert_eq!(m.max_tokens, 64_000);
    assert_eq!(m.cost.input, 3.0);
    assert_eq!(m.cost.output, 15.0);
    assert_eq!(m.cost.cache_read, 0.3);
    assert_eq!(m.cost.cache_write, 3.75);
    assert!(m.reasoning);
    assert_eq!(m.input, vec!["text".to_string(), "image".to_string()]);
    assert_eq!(m.name, "Claude Sonnet 4.5 (latest)");
    assert_eq!(m.api, "anthropic-messages");
}

/// The point of generating from data rather than typing: the windows are not
/// one number. Six distinct values across the kept providers, against the
/// single hardcoded 128_000 this replaces.
#[test]
fn the_catalog_is_not_one_number_wearing_many_coats() {
    let rt = ModelRuntime::with_defaults();
    let windows: std::collections::HashSet<u32> =
        rt.all().iter().map(|m| m.context_window).collect();
    assert!(
        windows.len() > 5,
        "expected a spread of real context windows, got {:?}",
        windows
    );
    assert!(
        windows.contains(&200_000) && windows.contains(&1_000_000),
        "including both the 200k and 1M families: {:?}",
        windows
    );
    assert!(
        !windows.contains(&128_000) || windows.len() > 1,
        "the old flat 128_000 must not be the only value"
    );
}

/// `resolve` used to return `None` for everything, so every
/// provider-scoped lookup in the crate got nothing.
#[test]
fn resolve_finds_a_model_by_provider_and_id() {
    let m = resolve("anthropic", "claude-opus-4-7").expect("must resolve");
    assert_eq!(m.id, "anthropic/claude-opus-4-7");
    assert_eq!(m.context_window, 1_000_000);

    assert!(
        resolve("anthropic", "no-such-model").is_none(),
        "and still reports absence rather than inventing one"
    );
    assert!(resolve("no-such-provider", "claude-opus-4-7").is_none());
}

/// `supportsTemperature` is declared per model in Pi's data
/// (`claude-opus-4-7`, `claude-opus-4-8`, `claude-opus-5`) and drives
/// whether Pi's Anthropic path sends temperature at all
/// (`anthropic-messages.ts:182`, `:1021`).
#[test]
fn per_model_temperature_support_survives() {
    assert!(
        !spec("anthropic/claude-opus-4-7").supports_temperature,
        "Pi declares supportsTemperature: false for this one"
    );
    assert!(
        spec("anthropic/claude-sonnet-4-5").supports_temperature,
        "and true for Sonnet, which does not declare it"
    );
}

/// The three models the old stub invented.
#[test]
fn the_invented_models_are_gone() {
    for id in [
        "cohere/command-r-plus",
        "deepseek/deepseek-chat",
        "groq/llama-3.3-70b",
    ] {
        assert!(
            resolve(id.split('/').next().unwrap(), id.split('/').nth(1).unwrap()).is_none(),
            "{id} was in the old stub and does not exist in Pi's catalog"
        );
    }
}

/// `from_models_json` used to ignore its argument and return the stub, so a
/// `--models` flag looked like it worked.
#[test]
fn from_models_json_actually_parses_its_input() {
    let custom = r#"[
      {"id":"acme/tiny","name":"Tiny","api":"anthropic-messages",
       "provider":"acme","reasoning":false,"input":["text"],
       "context_window":7777,"max_tokens":111,
       "cost":{"input":1.0,"output":2.0,"cache_read":0.0,"cache_write":0.0},
       "supports_temperature":true,"thinking_levels":[]}
    ]"#;
    let rt = ModelRuntime::from_models_json(custom);
    assert_eq!(rt.len(), 1, "exactly what was passed in, nothing else");
    let m = rt.find_by_id("acme/tiny").expect("must be present");
    assert_eq!(m.context_window, 7777);
    assert_eq!(m.max_tokens, 111);
    assert_eq!(m.cost.output, 2.0);
}

/// Malformed input should fall over rather than quietly fall back.
#[test]
fn a_malformed_catalog_is_an_error() {
    let r = std::panic::catch_unwind(|| ModelRuntime::from_models_json("{not json"));
    assert!(r.is_err(), "a broken catalog must not silently fall back");
}

/// Only providers nini can reach are carried. Listing 1267 models for 39
/// providers when six have an implementation would be a catalog that lies
/// about what is reachable.
#[test]
fn only_reachable_providers_are_carried() {
    let rt = ModelRuntime::with_defaults();
    let all = rt.all();
    let providers: std::collections::HashSet<String> =
        all.iter().map(|m| m.provider.clone()).collect();
    let expected: std::collections::HashSet<String> = [
        "anthropic",
        "openai",
        "deepseek",
        "groq",
        "mistral",
        "google",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(providers, expected);
    assert!(
        rt.len() > 50,
        "and it is a real catalog, not a handful: {}",
        rt.len()
    );
}
