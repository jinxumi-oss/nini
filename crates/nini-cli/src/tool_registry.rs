//! v0.8.2: stub — full implementation in Phase 1.3.

use nini_core::tool::ToolRegistry;

pub(crate) fn build_tools() -> ToolRegistry {
    unimplemented!("v0.8.2: tool_registry stub")
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn filter_tools(
    registry: ToolRegistry,
    tools: &[String],
    exclude_tools: &[String],
    no_tools: bool,
    no_builtin_tools: bool,
) -> ToolRegistry {
    let _ = (registry, tools, exclude_tools, no_tools, no_builtin_tools);
    unimplemented!("v0.8.2: tool_registry stub")
}