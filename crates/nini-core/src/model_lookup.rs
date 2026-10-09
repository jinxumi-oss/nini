//! Looking a model's facts up in the shipped catalog.

use crate::model_runtime::{resolve, ModelRuntime};

/// `contextWindow` for a `provider/model` string, if the catalog has it.
///
/// A bare id is accepted too, matched against any provider — the catalog is
/// small enough that the ambiguity does not matter for a window size, and
/// refusing to guess here would put the settings fallback in play for a
/// model whose real value we already hold.
pub fn context_window_for(model: &str) -> Option<u32> {
    if model.is_empty() {
        return None;
    }
    if model.contains('/') {
        if let Some((provider, id)) = model.split_once('/') {
            if let Some(m) = resolve(provider, id) {
                return Some(m.context_window);
            }
        }
    }
    ModelRuntime::with_defaults()
        .all()
        .into_iter()
        .find(|m| m.id.ends_with(&format!("/{model}")))
        .map(|m| m.context_window)
}

/// Cost in USD for one turn, mirroring Pi's `calculateCost`
/// (`pi-ai/src/models.ts:878`).
///
/// ```text
///   cost.input      = (rates.input      / 1e6) * usage.input
///   cost.output     = (rates.output     / 1e6) * usage.output
///   cost.cacheRead  = (rates.cacheRead  / 1e6) * usage.cacheRead
///   cost.cacheWrite = (rates.cacheWrite / 1e6) * usage.cacheWrite
///   cost.total      = the sum
/// ```
///
/// Pi splits cache writes into short and 1h-retention halves, charging
/// `input * 2` for the latter (`models.ts:889-895`). Only Anthropic reports
/// the split and nini's `Usage` has no field for it, so `longWrite` is zero
/// here — which is exactly the value Pi uses when `cacheWrite1h` is
/// absent. That is a real limitation of the provider layer, not a guess:
/// an Anthropic turn that wrote a 1h cache entry will be under-counted.
///
/// Rates are dollars per million tokens; the tier is chosen by total input
/// tokens (`input + cacheRead + cacheWrite`), taking the highest threshold
/// that applies.
pub fn calculate_cost(
    spec: &crate::model_runtime::ModelSpec,
    usage: &crate::provider::Usage,
) -> f64 {
    let input_tokens = usage.input_tokens as u64
        + usage.cache_read_tokens as u64
        + usage.cache_write_tokens as u64;

    let mut rates = spec.cost;
    let mut matched: i64 = -1;
    for tier in &spec.cost_tiers {
        if input_tokens > tier.input_tokens_above && tier.input_tokens_above as i64 > matched {
            rates = tier.cost;
            matched = tier.input_tokens_above as i64;
        }
    }

    per_million(rates.input, usage.input_tokens)
        + per_million(rates.output, usage.output_tokens)
        + per_million(rates.cache_read, usage.cache_read_tokens)
        + per_million(rates.cache_write, usage.cache_write_tokens)
}

fn per_million(rate: f64, tokens: u32) -> f64 {
    (rate / 1_000_000.0) * tokens as f64
}

/// Cost for a `provider/model` string, if the catalog has it.
///
/// `None` means "no price known", which is not the same as zero: a free
/// model and an unpriced one are different facts, and the status bar treats
/// them differently.
pub fn cost_for(model: &str) -> Option<crate::model_runtime::ModelCost> {
    find(model).map(|m| m.cost)
}

/// The catalog entry for a model string, bare or qualified.
pub fn find(model: &str) -> Option<crate::model_runtime::ModelSpec> {
    if model.is_empty() {
        return None;
    }
    let rt = ModelRuntime::with_defaults();
    if model.contains('/') {
        let (provider, id) = model.split_once('/').unwrap();
        if let Some(m) = rt.find_by_id(model) {
            return Some(m);
        }
        if let Some(m) = rt
            .all()
            .into_iter()
            .find(|m| m.provider == provider && m.id.ends_with(&format!("/{id}")))
        {
            return Some(m);
        }
        return None;
    }
    rt.all()
        .into_iter()
        .find(|m| m.id.ends_with(&format!("/{model}")))
}

/// Cost of one turn for a model string, from token counts.
///
/// The TUI calls this with the model it currently has selected, which is the
/// one that produced the turn. `None` from the lookup becomes `0.0` here:
/// an unknown model has no price to charge, and inventing one would put a
/// number on screen that no rate card supports. The status bar hides the
/// pill at zero, so "unknown" reads as absent rather than as free.
pub fn turn_cost(model: &str, input_tokens: u32, output_tokens: u32) -> f64 {
    let Some(spec) = find(model) else {
        return 0.0;
    };
    calculate_cost(
        &spec,
        &crate::provider::Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        },
    )
}
