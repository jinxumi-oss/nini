//! Oracle: the OpenAI Chat request nini puts on the wire.
//!
//! Ground truth is a capture, not a reading of Pi's source. Pi 0.84.2 was
//! pointed at a local mock endpoint and made ten real turns. Nine carried
//! a model request, and all nine had exactly this key set:
//!
//! ```text
//!   model, messages, stream, stream_options, store, max_completion_tokens, tools
//! ```
//!
//! Not `max_tokens`. `store` present and `false`. That is OpenAI itself.
//!
//! **What the capture does not reach.** Every captured request went to
//! OpenAI. The per-provider differences come from Pi's `getCompatOptions`
//! (`openai-completions.ts:1462-1502`), read from the reconstructed source
//! rather than captured, because there is no capture for a DeepSeek-shaped
//! endpoint. That provenance is marked at each use below rather than
//! blurred together.
//!
//! **What this file deliberately does not assert.** The nine captures had
//! no temperature configured, so there is no evidence about that case
//! either way. nini follows Pi's source, which sends it whenever it is
//! set, with no reasoning carve-out. An earlier version of this change
//! dropped temperature for reasoning models "because OpenAI rejects it" -
//! a divergence from the specification dressed as a safety measure, and
//! removed.

use nini_ai::openai::{MaxTokensField, OpenAiCompat};
use nini_core::provider::{ContentBlock, Message, Request, Role, ThinkingLevel, ToolSpec};

fn base_request() -> Request {
    Request {
        model: "test-model".into(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: "hi".into() }],
            timestamp: 0,
        }],
        tools: vec![ToolSpec {
            name: "read".into(),
            description: "read a file".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }],
        max_tokens: Some(8192),
        temperature: None,
        system: None,
        reasoning: None,
    }
}

fn openai_body(req: &Request) -> serde_json::Value {
    nini_ai::openai::build_request_for_test(req)
}

fn keys_of(v: &serde_json::Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

/// The captured key set, verbatim.
#[test]
fn the_key_set_matches_the_capture() {
    let captured = [
        "max_completion_tokens",
        "messages",
        "model",
        "store",
        "stream",
        "stream_options",
        "tools",
    ];
    assert_eq!(keys_of(&openai_body(&base_request())), captured);
}

/// The budget goes out as `max_completion_tokens`, and `max_tokens` does
/// not appear at all. Sending the older name to a model that takes the
/// newer one is a 400, not a cosmetic difference.
#[test]
fn the_budget_goes_out_as_max_completion_tokens() {
    let v = openai_body(&base_request());
    assert_eq!(v["max_completion_tokens"], 8192, "{}", v);
    assert!(v.get("max_tokens").is_none(), "{}", v);
}

/// `store: false` was on every capture. Present-and-false, not merely
/// absent: the field's absence relies on somebody else's default.
#[test]
fn store_is_explicitly_false() {
    let v = openai_body(&base_request());
    assert_eq!(v["store"], false, "{}", v);
}

/// Source-derived, not captured: Pi selects the field per provider.
///
/// `maxTokensField: useMaxTokens ? "max_tokens" : "max_completion_tokens"`
/// (`openai-completions.ts:1502`), with DeepSeek, Moonshot, Together,
/// NVIDIA, Chutes, Z.ai, Ant Ling and the Cloudflare AI gateway in the
/// `max_tokens` group (`:1480`).
#[test]
fn a_compat_endpoint_gets_max_tokens_and_no_store() {
    let v = nini_ai::openai::build_request_for_compat(
        &base_request(),
        OpenAiCompat::uses_max_tokens_field(),
    );
    assert_eq!(
        v["max_tokens"], 8192,
        "Pi puts DeepSeek & co. on the older field name: {}",
        v
    );
    assert!(
        v.get("max_completion_tokens").is_none(),
        "and not on both: {}",
        v
    );
    assert!(
        v.get("store").is_none(),
        "Pi excludes the same providers from `store` \\
         (`supportsStore: !isNonStandard`, :1499) - a non-standard endpoint \
         may reject the field outright: {}",
        v
    );
}

/// The two groups must be genuinely exclusive, in both directions. A
/// descriptor that set both fields would be worse than either.
#[test]
fn the_two_token_fields_are_never_both_present() {
    for (compat, expect_new, expect_old) in [
        (OpenAiCompat::default(), true, false),
        (OpenAiCompat::uses_max_tokens_field(), false, true),
    ] {
        let v = nini_ai::openai::build_request_for_compat(&base_request(), compat);
        assert_eq!(
            v.get("max_completion_tokens").is_some(),
            expect_new,
            "{:?} on max_completion_tokens: {}",
            compat.max_tokens_field,
            v
        );
        assert_eq!(
            v.get("max_tokens").is_some(),
            expect_old,
            "{:?} on max_tokens: {}",
            compat.max_tokens_field,
            v
        );
    }
    assert_eq!(
        OpenAiCompat::default().max_tokens_field,
        MaxTokensField::MaxCompletionTokens
    );
}

/// Source-derived: Pi sends temperature whenever it is set, with no
/// reasoning carve-out (`openai-completions.ts:722`).
///
/// This is the case the captures cannot reach, and it is the case where
/// nini was briefly wrong in the other direction.
#[test]
fn a_configured_temperature_is_sent_even_with_reasoning_on() {
    let mut req = base_request();
    req.temperature = Some(0.7);
    req.reasoning = Some(ThinkingLevel::High);
    let v = openai_body(&req);

    assert_eq!(v["reasoning_effort"], "high", "{}", v);
    // Compared with a tolerance: `Request.temperature` is an `f32` and
    // serde_json widens it to `f64`, so 0.7f32 arrives as
    // 0.699999988079071. An exact compare would fail for a reason that has
    // nothing to do with the wire contract.
    let got = v["temperature"]
        .as_f64()
        .expect("temperature must be a number");
    assert!(
        (got - 0.7).abs() < 1e-6,
        "Pi sends temperature alongside reasoning_effort; dropping it \
         would be a divergence from the specification, not a safety \
         measure: {got} in {}",
        v
    );
}

#[test]
fn an_unset_temperature_is_omitted() {
    let v = openai_body(&base_request());
    assert!(v.get("temperature").is_none(), "{}", v);
}

/// `off` is omission, not the literal string. The OpenAI API has no such
/// value, and Pi expresses it as absence
/// (`openai-completions.ts:626`).
#[test]
fn reasoning_off_is_omitted_rather_than_sent_as_a_literal() {
    let mut req = base_request();
    req.reasoning = Some(ThinkingLevel::Off);
    let v = openai_body(&req);
    assert!(
        v.get("reasoning_effort").is_none(),
        "`off` must not appear on the wire: {}",
        v
    );
}
