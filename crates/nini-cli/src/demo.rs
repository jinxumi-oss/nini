//! v0.8.2: stub — full implementation in Phase 3.

use std::path::Path;

pub(crate) async fn run_demo(
    _task: &str,
    _provider: &str,
    _model: &str,
    _fallback_keys: &[String],
    _fallback_base_urls: &[String],
) -> anyhow::Result<()> {
    unimplemented!("v0.8.2: demo stub")
}

pub(crate) async fn run_print(
    _task: &str,
    _provider: &str,
    _model: &str,
    _fallback_keys: &[String],
    _fallback_base_urls: &[String],
) -> anyhow::Result<()> {
    unimplemented!("v0.8.2: demo stub")
}

pub(crate) fn demo_fix_todos_turns(_cwd: &Path) -> Vec<Vec<nini_ai::fixture::FixtureTurn>> {
    Vec::new()
}

pub(crate) fn demo_simple_turns(_task: &str) -> Vec<Vec<nini_ai::fixture::FixtureTurn>> {
    Vec::new()
}

pub(crate) fn find_first_file_with_todo(_cwd: &Path) -> Option<String> {
    None
}

pub(crate) fn cli_entry_legacy(_entry: &nini_core::SessionEntry) -> Option<nini_core::AgentMessage> {
    None
}