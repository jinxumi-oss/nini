//! Oracle: the Anthropic request's `temperature`.
//!
//! Pi, `anthropic-messages.ts:1021`:
//!
//! ```ts
//! if (options?.temperature !== undefined
//!     && !options?.thinkingEnabled
//!     && compat.supportsTemperature) {
//!     params.temperature = options.temperature;
//! }
//! ```
//!
//! and the flag defaults to true per model
//! (`model.compat?.supportsTemperature ?? true`, `:182`). Real model data
//! carries it: `claude-opus-4-7` and `claude-opus-5` declare
//! `supportsTemperature: false`.
//!
//! nini sent `temperature` unconditionally. The reasoning half of that rule
//! was missed because an earlier change concluded from Pi's *OpenAI* path -
//! where temperature really is sent unconditionally - that Pi sends it
//! always. One path's rule is not another's rule.

use nini_core::provider::{ContentBlock, Message, Request, Role, ThinkingLevel};

fn req(temperature: Option<f32>, reasoning: Option<ThinkingLevel>) -> Request {
    Request {
        model: "claude-sonnet-4-5".into(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: "hi".into() }],
            timestamp: 0,
        }],
        tools: vec![],
        max_tokens: Some(1024),
        temperature,
        system: None,
        reasoning,
    }
}

fn body(r: &Request) -> serde_json::Value {
    serde_json::from_str(&nini_ai::anthropic::build_request_body(r).expect("must build"))
        .expect("must be JSON")
}

/// The captured rule. With thinking off, temperature goes out.
#[test]
fn without_thinking_a_temperature_is_sent() {
    let v = body(&req(Some(0.5), None));
    let got = v["temperature"]
        .as_f64()
        .expect("temperature must be present");
    assert!((got - 0.5).abs() < 1e-6, "{got} in {v}");
}

/// The rule that was missed: thinking on suppresses temperature.
///
/// `!options?.thinkingEnabled` is half of Pi's condition
/// (`anthropic-messages.ts:1021`). nini ignored it, so an extended-thinking
/// request carried a sampling parameter alongside a thinking budget.
#[test]
fn with_thinking_on_no_temperature_is_sent() {
    let v = body(&req(Some(0.5), Some(ThinkingLevel::High)));
    assert!(
        v.get("temperature").is_none(),
        "thinking and temperature must not travel together: {}",
        v
    );
    // The thinking block itself must still be there - this is about
    // temperature, not about dropping the request's purpose.
    assert!(
        v.get("thinking").is_some(),
        "thinking must still be requested: {}",
        v
    );
}

/// `off` is omission for thinking too, so temperature comes back.
#[test]
fn thinking_off_restores_temperature() {
    let v = body(&req(Some(0.5), Some(ThinkingLevel::Off)));
    let got = v["temperature"]
        .as_f64()
        .expect("temperature must be present");
    assert!((got - 0.5).abs() < 1e-6, "{got} in {v}");
}

/// No temperature configured means none sent, in either thinking state.
#[test]
fn an_unset_temperature_is_omitted_either_way() {
    for reasoning in [None, Some(ThinkingLevel::High)] {
        let v = body(&req(None, reasoning));
        assert!(
            v.get("temperature").is_none(),
            "{reasoning:?} must not conjure a temperature: {}",
            v
        );
    }
}
