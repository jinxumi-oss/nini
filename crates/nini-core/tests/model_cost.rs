//! Oracle: the cost the status bar shows.
//!
//! `run_state.cost_usd` was structurally always zero: `app.rs` pushed
//! `AgentEventLite::Usage(input, output, 0.0)`, and `render.rs` shows the
//! cost pill only when `cost_usd > 0.0`. A field that exists and is always
//! zero is worse than an absent feature.
//!
//! The formula is Pi's `calculateCost` (`pi-ai/src/models.ts:878`), and
//! the rates are Pi's own, from the same `providers/data/*.json` the
//! catalog is generated from:
//!
//! ```text
//!   cost.input      = (rates.input      / 1e6) * usage.input
//!   cost.output     = (rates.output     / 1e6) * usage.output
//!   cost.cacheRead  = (rates.cacheRead  / 1e6) * usage.cacheRead
//!   cost.cacheWrite = (rates.cacheWrite / 1e6) * usage.cacheWrite
//!   cost.total      = the sum
//! ```
//!
//! Rates are dollars per million tokens (`types.ts:779`).

use nini_core::model_lookup::{calculate_cost, cost_for, find};
use nini_core::model_runtime::ModelRuntime;
use nini_core::provider::Usage;

fn usage(input: u32, output: u32, cache_read: u32, cache_write: u32) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Sonnet's rates are Pi's: 3 / 15 / 0.3 / 3.75 per million.
#[test]
fn the_price_comes_from_the_catalog() {
    let m = find("anthropic/claude-sonnet-4-5").expect("must be in the catalog");
    assert_eq!(m.cost.input, 3.0);
    assert_eq!(m.cost.output, 15.0);
    assert_eq!(m.cost.cache_read, 0.3);
    assert_eq!(m.cost.cache_write, 3.75);
}

/// One million input tokens on Sonnet costs exactly its input rate.
#[test]
fn a_million_input_tokens_costs_the_input_rate() {
    let m = find("anthropic/claude-sonnet-4-5").unwrap();
    let c = calculate_cost(&m, &usage(1_000_000, 0, 0, 0));
    assert!(near(c, 3.0), "{c} != 3.0");
}

/// The whole shape at once: 1M in, 1M out, 1M read, 1M written.
/// 3 + 15 + 0.3 + 3.75 = 22.05.
#[test]
fn all_four_channels_are_charged() {
    let m = find("anthropic/claude-sonnet-4-5").unwrap();
    let c = calculate_cost(&m, &usage(1_000_000, 1_000_000, 1_000_000, 1_000_000));
    assert!(near(c, 22.05), "{c} != 22.05");
}

/// Rates are per million, not per token. A single token must cost
/// `rate / 1e6`; getting the divisor wrong is the classic mistake and is a
/// million-fold error, so it is pinned directly.
#[test]
fn rates_are_per_million_not_per_token() {
    let m = find("anthropic/claude-sonnet-4-5").unwrap();
    let one = calculate_cost(&m, &usage(1, 0, 0, 0));
    assert!(near(one, 3.0 / 1_000_000.0), "{one}");
}

/// Zero usage costs nothing, and the absence of a price is not the same as
/// a price of zero.
#[test]
fn zero_usage_costs_nothing_but_an_unknown_model_has_no_price() {
    let m = find("anthropic/claude-sonnet-4-5").unwrap();
    assert_eq!(calculate_cost(&m, &usage(0, 0, 0, 0)), 0.0);

    assert!(
        cost_for("acme/nobody-heard-of-it").is_none(),
        "an unpriced model is a different fact from a free one"
    );
    assert!(
        cost_for("anthropic/claude-sonnet-4-5").is_some(),
        "and a known one must resolve"
    );
}

/// Tiered pricing, selected by total input tokens. No model nini currently
/// reaches has tiers - 21 of Pi's 1267 do - so this drives a synthetic
/// entry. The point is that a flat rate cannot silently under-price a tiered
/// model once the catalog grows to include one.
#[test]
fn a_tier_above_the_threshold_is_charged_instead_of_the_base_rate() {
    let catalog = r#"[
      {"id":"acme/tiered","name":"Tiered","api":"anthropic-messages",
       "provider":"acme","reasoning":false,"input":["text"],
       "context_window":1000000,"max_tokens":64000,
       "cost":{"input":1.0,"output":2.0,"cache_read":0.1,"cache_write":1.0},
       "supports_temperature":true,"thinking_levels":[],
       "cost_tiers":[{"input_tokens_above":200000,
                      "input":2.0,"output":4.0,
                      "cache_read":0.2,"cache_write":2.0}]}
    ]"#;
    let rt = ModelRuntime::from_models_json(catalog);
    let m = rt.find_by_id("acme/tiered").expect("must load");

    // Below the threshold: base rates.
    let small = calculate_cost(&m, &usage(100_000, 0, 0, 0));
    assert!(near(small, 1.0 * 100_000.0 / 1e6), "{small}");

    // Above it: the tier, on every channel Pi prices from it.
    let big = calculate_cost(&m, &usage(300_000, 300_000, 0, 0));
    assert!(
        near(big, 2.0 * 300_000.0 / 1e6 + 4.0 * 300_000.0 / 1e6),
        "{big}"
    );
}

/// The threshold counts input *plus* cache traffic, as Pi does
/// (`const inputTokens = usage.input + usage.cacheRead + usage.cacheWrite`).
#[test]
fn the_tier_threshold_counts_cache_traffic() {
    let catalog = r#"[
      {"id":"acme/tiered2","name":"Tiered2","api":"anthropic-messages",
       "provider":"acme","reasoning":false,"input":["text"],
       "context_window":1000000,"max_tokens":64000,
       "cost":{"input":1.0,"output":2.0,"cache_read":0.1,"cache_write":1.0},
       "supports_temperature":true,"thinking_levels":[],
       "cost_tiers":[{"input_tokens_above":200000,
                      "input":2.0,"output":4.0,
                      "cache_read":0.2,"cache_write":2.0}]}
    ]"#;
    let m = ModelRuntime::from_models_json(catalog)
        .find_by_id("acme/tiered2")
        .unwrap();

    // 150k input alone is under; adding cache reads pushes it over.
    let input_only = calculate_cost(&m, &usage(150_000, 0, 0, 0));
    let with_cache = calculate_cost(&m, &usage(150_000, 0, 60_000, 0));
    assert!(
        near(input_only, 0.15),
        "150k at the base rate: {input_only}"
    );
    assert!(
        with_cache > input_only * 2.0,
        "cache reads must count toward the threshold: {with_cache} vs \
         {input_only}"
    );
}

/// A bare model id resolves, because nini names models both ways.
#[test]
fn a_bare_model_id_resolves_its_price() {
    assert!(cost_for("claude-sonnet-4-5").is_some());
    assert_eq!(
        cost_for("claude-sonnet-4-5"),
        cost_for("anthropic/claude-sonnet-4-5")
    );
}
