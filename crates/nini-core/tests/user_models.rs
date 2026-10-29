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
use nini_core::settings::{load_models_json, load_user_models_json_reporting, ModelsJson};
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

/// The provider's **connection** settings are read in Pi's spelling too.
///
/// `baseUrl` and `apiKey` are the two fields that decide where a request
/// goes and what it carries. Pi writes them in camelCase; nini's struct
/// had only the snake_case spelling and no alias, so serde dropped both
/// keys on the floor — the provider was in the catalogue and its
/// connection settings were gone.
#[test]
fn the_provider_connection_settings_are_read_in_pis_spelling() {
    let cfg: ModelsJson = serde_json::from_str(pi_style_models_json()).expect("models.json parses");
    let provider = cfg.providers.get("aiio-failover").expect("provider");
    assert_eq!(
        provider.base_url.as_deref(),
        Some("http://127.0.0.1:9877/v1"),
        "Pi writes `baseUrl`; nini must read it, not ignore it"
    );
    assert_eq!(
        provider.api_key.as_deref(),
        Some("proxy-handles-auth"),
        "Pi writes `apiKey`; nini must read it, not ignore it"
    );
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

/// A `models.json` with comments in it is read, like Pi reads it.
///
/// Pi: `model-config.js:230` parses `stripJsonComments(stripBom(content))`
/// — a user who writes a comment in their `models.json` has a working
/// setup in Pi. nini used to hand the raw text to serde, so the same file
/// was "Failed to parse models.json" here.
#[test]
fn a_models_json_with_comments_is_read() {
    let guard = nini_core::paths::agent_lock();
    let mut f =
        std::fs::File::create(guard.path().join("models.json")).expect("create models.json");
    // A comment, a trailing comma, and a byte order mark — all three are
    // things Pi accepts.
    f.write_all(b"\xef\xbb\xbf{\n  // a comment\n  \"providers\": {\n")
        .expect("write");
    f.write_all(b"    \"gw\": {\n      \"baseUrl\": \"http://x.invalid/v1\",\n")
        .expect("write");
    f.write_all(b"      \"api\": \"openai-completions\",\n      \"models\": [\n")
        .expect("write");
    f.write_all(b"        { \"id\": \"m\" },\n      ],\n    },\n  }\n}\n")
        .expect("write");

    let (cfg, error) = load_user_models_json_reporting();
    assert!(
        error.is_none(),
        "Pi reads this file, so nini must too: {error:?}"
    );
    let provider = cfg.providers.get("gw").expect("the provider is read");
    assert_eq!(provider.base_url.as_deref(), Some("http://x.invalid/v1"));
    assert_eq!(provider.api.as_deref(), Some("openai-completions"));
    assert_eq!(
        provider.models.first().and_then(|m| m.id.as_deref()),
        Some("m")
    );
}

/// The **catalogue** side reads a commented file too.
///
/// `load_models_json` is the other reader: it is what `--list-models`
/// shows. It has to make the same decisions as the user-level reader,
/// or a user with a commented file gets a provider that Pi lists and
/// nini does not.
#[test]
fn the_catalogue_reads_a_models_json_with_comments() {
    let guard = nini_core::paths::agent_lock();
    let mut f =
        std::fs::File::create(guard.path().join("models.json")).expect("create models.json");
    writeln!(
        f,
        r#"{{
  // the user's own comment
  "providers": {{
    "my-gw": {{
      "baseUrl": "http://x.invalid/v1",
      "api": "openai-completions",
      "models": [ {{ "id": "gw-model" }} ],
    }},
  }},
}}"#
    )
    .expect("write");

    let cfg = load_models_json(std::path::Path::new("/tmp"));
    let specs = user_model_specs(&cfg);
    assert!(
        specs.iter().any(|s| s.id == "my-gw/gw-model"),
        "the model the user declared must be listed: {specs:?}"
    );
}

/// A `models.json` that cannot be parsed is **reported**, not dropped.
///
/// Pi: `model-config.js:233` turns a parse error into
/// `Failed to parse models.json: …`. nini used to discard the file
/// without a word, which left a user looking at a provider that was
/// simply not there.
#[test]
fn a_models_json_that_does_not_parse_is_reported() {
    let guard = nini_core::paths::agent_lock();
    let mut f =
        std::fs::File::create(guard.path().join("models.json")).expect("create models.json");
    writeln!(f, "{{ this is not json").expect("write");

    let (cfg, error) = load_user_models_json_reporting();
    assert!(
        cfg.providers.is_empty(),
        "a file that does not parse contributes no provider"
    );
    let error = error.expect("a broken file must be reported");
    assert!(
        error.starts_with("Failed to parse models.json:"),
        "Pi's sentence, verbatim: {error}"
    );
}

/// A `models.json` that does not exist is not an error at all.
///
/// Pi: `model-config.js:215-224` — no file, no configuration, no
/// complaint.
#[test]
fn an_absent_models_json_is_not_an_error() {
    let guard = nini_core::paths::agent_lock();
    let _ = std::fs::remove_file(guard.path().join("models.json"));

    let (_cfg, error) = load_user_models_json_reporting();
    assert!(
        error.is_none(),
        "a user who has no models.json has nothing to be told: {error:?}"
    );
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
