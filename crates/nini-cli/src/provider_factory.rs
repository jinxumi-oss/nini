//! v0.8.2: stub — full implementation in Phase 1.2.

use std::sync::Arc;

pub(crate) fn build_provider(
    _name: &str,
    _scripted_turns: Vec<Vec<nini_ai::fixture::FixtureTurn>>,
    _fallback_keys: &[String],
    _fallback_base_urls: &[String],
) -> anyhow::Result<Arc<dyn nini_core::provider::Provider>> {
    unimplemented!("v0.8.2: provider_factory stub")
}

pub(crate) fn parse_scripted_turns(_raw: &str) -> Option<Vec<Vec<nini_ai::fixture::FixtureTurn>>> {
    None
}

pub(crate) fn make_anthropic(_key: &str, _base: Option<&str>) -> nini_ai::anthropic::AnthropicProvider {
    unimplemented!("v0.8.2: provider_factory stub")
}