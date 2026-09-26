//! v0.8.2: Tool registry factory + CLI filtering.
//!
//! Two functions:
//!   * `build_tools()` — registers all 6 built-in tools
//!   * `filter_tools()` — applies `--tools` / `--exclude-tools` /
//!     `--no-tools` / `--no-builtin-tools` CLI flags to a registry
//!
//! Pure factory + pure filter; both trivially testable.

use std::sync::Arc;

use nini_core::tool::{Tool, ToolRegistry};
use nini_tools::{BashTool, EditTool, FindTool, GrepTool, ReadTool, WriteTool};

/// Register all 6 built-in tools in their canonical order.
pub(crate) fn build_tools() -> ToolRegistry {
    let bash: Arc<dyn Tool> = Arc::new(BashTool::new());
    let read: Arc<dyn Tool> = Arc::new(ReadTool::new());
    let write: Arc<dyn Tool> = Arc::new(WriteTool::new());
    let edit: Arc<dyn Tool> = Arc::new(EditTool::new());
    let grep: Arc<dyn Tool> = Arc::new(GrepTool::new());
    let find: Arc<dyn Tool> = Arc::new(FindTool::new());
    let mut reg = ToolRegistry::new();
    reg.register_mut(bash);
    reg.register_mut(read);
    reg.register_mut(write);
    reg.register_mut(edit);
    reg.register_mut(grep);
    reg.register_mut(find);
    reg
}

/// Apply CLI tool filtering (`--tools`, `--exclude-tools`, `--no-tools`,
/// `--no-builtin-tools`) to a registry.
///
/// Filtering rules (in order):
///   1. `--no-tools`        → drop everything
///   2. `--no-builtin-tools` → drop the 6 built-in tools
///   3. `--tools a,b,c`     → keep only the named tools
///   4. `--exclude-tools a` → drop the named tools
///
/// Multiple flags combine as intersection.
pub(crate) fn filter_tools(
    registry: ToolRegistry,
    allow: &[String],
    deny: &[String],
    no_tools: bool,
    no_builtin: bool,
) -> ToolRegistry {
    let builtin = ["bash", "read", "write", "edit", "grep", "find"];
    let tools: Vec<(String, Arc<dyn Tool>)> = registry
        .tools()
        .map(|t| (t.name().to_string(), t))
        .collect();
    let mut kept: Vec<Arc<dyn Tool>> = Vec::new();
    for (name, tool) in tools {
        if no_tools {
            continue;
        }
        if no_builtin && builtin.contains(&name.as_str()) {
            continue;
        }
        if !allow.is_empty() && !allow.iter().any(|a| a == &name) {
            continue;
        }
        if deny.iter().any(|d| d == &name) {
            continue;
        }
        kept.push(tool);
    }
    let mut out = ToolRegistry::new();
    for tool in kept {
        out.register_mut(tool);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_tools_registers_all_six() {
        let reg = build_tools();
        let names: Vec<&str> = reg.tools().map(|t| t.name()).collect();
        assert_eq!(names.len(), 6, "expected 6 built-in tools");
        for n in &["bash", "read", "write", "edit", "grep", "find"] {
            assert!(names.contains(&n), "missing tool: {n}");
        }
    }

    #[test]
    fn filter_tools_no_tools_drops_all() {
        let reg = build_tools();
        let out = filter_tools(reg, &[], &[], true, false);
        assert_eq!(out.tools().count(), 0);
    }

    #[test]
    fn filter_tools_no_builtin_drops_only_builtins() {
        let reg = build_tools();
        let out = filter_tools(reg, &[], &[], false, true);
        assert_eq!(out.tools().count(), 0, "all 6 are builtin");
    }

    #[test]
    fn filter_tools_allow_list_filters_in() {
        let reg = build_tools();
        let allow = vec!["read".to_string()];
        let out = filter_tools(reg, &allow, &[], false, false);
        let names: Vec<&str> = out.tools().map(|t| t.name()).collect();
        assert_eq!(names, vec!["read"]);
    }

    #[test]
    fn filter_tools_deny_list_drops_named() {
        let reg = build_tools();
        let deny = vec!["bash".to_string()];
        let out = filter_tools(reg, &[], &deny, false, false);
        let names: Vec<&str> = out.tools().map(|t| t.name()).collect();
        assert_eq!(names.len(), 5);
        assert!(!names.contains(&"bash"));
    }

    #[test]
    fn filter_tools_combined_intersection() {
        let reg = build_tools();
        let allow = vec!["bash".into(), "read".into()];
        let deny = vec!["read".into()];
        let out = filter_tools(reg, &allow, &deny, false, false);
        let names: Vec<&str> = out.tools().map(|t| t.name()).collect();
        assert_eq!(names, vec!["bash"], "deny wins in intersection");
    }
}