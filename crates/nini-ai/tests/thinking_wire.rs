//! Oracle: the thinking level reaches the provider wire.
//!
//! v0.10 (R1f). nini's `/thinking` command validated
//! `off|minimal|low|medium|high|xhigh|max` — exactly Pi's `ModelThinkingLevel`
//! (`pi-ai/src/types.ts:82-83`) — the TUI showed the current level, and
//! `settings.thinking_level` was persisted to `settings.json`. None of that
//! reached a model: `Request` (`provider.rs`) had no field to carry it, so
//! the value was **recorded, displayed, persisted, and never transmitted**.
//!
//! ## Wire shapes, from Pi
//!
//! ```ts
//! // anthropic-messages.ts:1050-1072, :824-826
//! if (!options?.reasoning) → thinkingEnabled: false
//!   thinking requested  → params.thinking = { type: "enabled", budget_tokens, display }
//!   explicitly off     → params.thinking = { type: "disabled" }
//!
//! // openai-completions.ts:626 — `off` is OMISSION, not a literal "off"
//! const reasoningEffort = clampedReasoning === "off" ? undefined : clampedReasoning;
//
//! // openai-responses.ts:206-210 — the Responses API nests it
//! { reasoning: { effort } }
//! ```
//!
//! The three builder sites in nini are `anthropic.rs`, `openai.rs` and
//! `openai_responses.rs`. `openai.rs` also covers every brand routed
//! through `OpenAiCompatProvider` — `openai_compat`, `mistral`, `groq`,
//! `google`, `cohere`, `deepseek` — so 9 of nini's 10 providers are reached
//! by one edit.

use nini_core::provider::{ContentBlock, Request, Role, ThinkingLevel};

fn req(reasoning: Option<ThinkingLevel>) -> Request {
    Request {
        model: "m".into(),
        messages: vec![nini_core::provider::Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: "hi".into() }],
            timestamp: 0,
        }],
        tools: vec![],
        max_tokens: Some(1024),
        temperature: None,
        system: None,
        reasoning,
    }
}

fn body_anthropic(r: &Request) -> serde_json::Value {
    serde_json::from_str(&nini_ai::anthropic::build_request_body(r).unwrap()).unwrap()
}

fn body_openai_chat(r: &Request) -> serde_json::Value {
    serde_json::from_str(&nini_ai::openai::build_request_body(r).unwrap()).unwrap()
}

fn body_openai_responses(r: &Request) -> serde_json::Value {
    serde_json::from_str(&nini_ai::openai_responses::build_request_body(r).unwrap()).unwrap()
}

// =====================================================================
// The level must be on the wire at all
// =====================================================================

#[test]
fn every_level_reaches_the_anthropic_wire() {
    for lvl in [
        ThinkingLevel::Minimal,
        ThinkingLevel::Low,
        ThinkingLevel::Medium,
        ThinkingLevel::High,
        ThinkingLevel::XHigh,
        ThinkingLevel::Max,
    ] {
        let b = body_anthropic(&req(Some(lvl)));
        let t = &b["thinking"];
        assert_eq!(t["type"], "enabled", "{lvl} should enable thinking: {b}");
        assert_eq!(
            t["budget_tokens"].as_u64(),
            Some(lvl.budget_tokens() as u64),
            "{lvl} should carry its own budget"
        );
    }
}

#[test]
fn distinct_levels_produce_distinct_budgets() {
    // A `/thinking max` that sends the same budget as `/thinking low` is
    // "recorded but not wired" wearing a new hat.
    let a = body_anthropic(&req(Some(ThinkingLevel::Low)))["thinking"]["budget_tokens"].clone();
    let b = body_anthropic(&req(Some(ThinkingLevel::Max)))["thinking"]["budget_tokens"].clone();
    assert_ne!(
        a, b,
        "low and max must not collapse to the same budget: {a} vs {b}"
    );
}

#[test]
fn off_disables_explicitly_and_none_is_omitted() {
    // Pi sends `{type:"disabled"}` for off and omits the field when the
    // caller expressed no opinion. Those are different requests.
    let off = body_anthropic(&req(Some(ThinkingLevel::Off)));
    assert_eq!(off["thinking"]["type"], "disabled", "{off}");

    let none = body_anthropic(&req(None));
    assert!(
        none.get("thinking").is_none(),
        "None must omit the field entirely, not send a default: {none}"
    );
}

// =====================================================================
// OpenAI chat completions — also covers the 6 compat brands
// =====================================================================

#[test]
fn reasoning_effort_reaches_the_openai_chat_wire() {
    for lvl in [
        ThinkingLevel::Low,
        ThinkingLevel::High,
        ThinkingLevel::XHigh,
    ] {
        let b = body_openai_chat(&req(Some(lvl)));
        assert_eq!(
            b["reasoning_effort"].as_str(),
            Some(lvl.as_str()),
            "{lvl} should map to reasoning_effort: {b}"
        );
    }
}

#[test]
fn openai_chat_omits_reasoning_effort_for_off_and_none() {
    // `reasoning_effort: "off"` is not a value the OpenAI API accepts —
    // Pi turns it into omission (`openai-completions.ts:626`).
    for r in [req(Some(ThinkingLevel::Off)), req(None)] {
        let b = body_openai_chat(&r);
        assert!(
            b.get("reasoning_effort").is_none(),
            "off/None must omit reasoning_effort, not send a literal: {b}"
        );
    }
}

// =====================================================================
// OpenAI Responses — nested object
// =====================================================================

#[test]
fn responses_nests_reasoning_under_effort() {
    let b = body_openai_responses(&req(Some(ThinkingLevel::High)));
    assert_eq!(b["reasoning"]["effort"].as_str(), Some("high"), "{b}");
}

#[test]
fn responses_omits_reasoning_for_off_and_none() {
    for r in [req(Some(ThinkingLevel::Off)), req(None)] {
        let b = body_openai_responses(&r);
        assert!(b.get("reasoning").is_none(), "{b}");
    }
}

// =====================================================================
// Vocabulary and budgets
// =====================================================================

#[test]
fn vocabulary_matches_pi_exactly() {
    // Pi: ThinkingLevel = minimal|low|medium|high|xhigh|max, plus "off".
    assert_eq!(
        ThinkingLevel::ALL,
        &["off", "minimal", "low", "medium", "high", "xhigh", "max"]
    );
    for s in ThinkingLevel::ALL {
        assert_eq!(
            s.parse::<ThinkingLevel>().unwrap().as_str(),
            *s,
            "round-trip failed for {s}"
        );
    }
    assert!("verbose".parse::<ThinkingLevel>().is_err());
}

#[test]
fn run_config_carries_the_level_into_the_request() {
    // End of the chain: settings → RunConfig → Request. If this breaks, the
    // value is persisted and still never sent.
    use nini_core::RunConfig;
    let cfg = RunConfig {
        reasoning: Some(ThinkingLevel::High),
        ..RunConfig::new("m")
    };
    let r = nini_core::agent::build_request_for_test(&cfg, &[], &[]);
    assert_eq!(r.reasoning, Some(ThinkingLevel::High));
}
