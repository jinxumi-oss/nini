//! v0.8.2: Tool registry factory + CLI filtering.
//!
//! Two functions:
//!   * `build_tools()` — registers all 7 built-in tools
//!   * `filter_tools()` — applies `--tools` / `--exclude-tools` /
//!     `--no-tools` / `--no-builtin-tools` CLI flags to a registry
//!
//! Pure factory + pure filter; both trivially testable.

use std::sync::Arc;

use nini_core::tool::{Tool, ToolRegistry};
use nini_tools::{BashTool, EditTool, FindTool, GrepTool, LsTool, ReadTool, WriteTool};

/// Register all 7 built-in tools in their canonical order.
///
/// v0.10.8 (T2): Pi 0.85.1 有 8 个内置工具，nini 此前 6 个；
/// 本次补上 `ls`（`powershell` 是 Windows 专用，有意不做，
/// 见 README known limitations）。
pub(crate) fn build_tools() -> ToolRegistry {
    let bash: Arc<dyn Tool> = Arc::new(BashTool::new());
    let read: Arc<dyn Tool> = Arc::new(ReadTool::new());
    let write: Arc<dyn Tool> = Arc::new(WriteTool::new());
    let edit: Arc<dyn Tool> = Arc::new(EditTool::new());
    let grep: Arc<dyn Tool> = Arc::new(GrepTool::new());
    let find: Arc<dyn Tool> = Arc::new(FindTool::new());
    let ls: Arc<dyn Tool> = Arc::new(LsTool::new());
    let mut reg = ToolRegistry::new();
    reg.register_mut(bash);
    reg.register_mut(read);
    reg.register_mut(write);
    reg.register_mut(edit);
    reg.register_mut(grep);
    reg.register_mut(find);
    reg.register_mut(ls);
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
    let builtin = ["bash", "read", "write", "edit", "grep", "find", "ls"];
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
    fn build_tools_registers_all_seven() {
        let reg = build_tools();
        let names: Vec<&str> = reg.tools().map(|t| t.name()).collect();
        assert_eq!(names.len(), 7, "expected 7 built-in tools");
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
        assert_eq!(out.tools().count(), 0, "all 7 are builtin");
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
        assert_eq!(names.len(), 6);
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
// v0.8.6: cross-cutting regression — every built-in tool must ship a
// `system_prompt_contribution()` so the model sees its snippet in the
// system prompt. If a new tool is wired into build_tools() without a
// contribution, this test catches the gap before release.
#[test]
fn every_built_in_tool_has_system_prompt_contribution() {
    use nini_core::settings::Settings;
    let reg = build_tools();
    let names: Vec<&str> = reg.tools().map(|t| t.name()).collect();
    assert_eq!(
        names.len(),
        7,
        "build_tools() must register exactly 7 tools (current: {names:?})"
    );
    for name in &names {
        let tools_for_prompt = build_tools();
        // v0.10.8 (T3): 工具段现在由 **agent** 统一拼一次
        // （nini-core::tool::build_system_prompt_with_contributions），
        // CLI 的 prompt_setup 只拼基座——所以这条断言要问对地方：
        // 「每个工具的 snippet 出现在模型收到的 prompt 里」。
        let prompt = nini_core::tool::build_system_prompt_with_contributions(
            Some(&crate::prompt_setup::settings_to_system_prompt(
                &Settings::default(),
                std::path::Path::new("/tmp/proj"),
                &tools_for_prompt,
                "",
            )),
            &tools_for_prompt,
        )
        .unwrap_or_default();
        let line = format!("- {name}:");
        assert!(
            prompt.contains(&line),
            "tool '{name}' missing from system prompt snippets; \
             did it implement system_prompt_contribution()?\n\
             full prompt:\n{prompt}"
        );
    }
}

#[test]
fn build_system_prompt_prompt_sections_sorted_alphabetically() {
    use nini_core::settings::Settings;
    let reg = build_tools();
    let prompt = crate::prompt_setup::settings_to_system_prompt(
        &Settings::default(),
        std::path::Path::new("/tmp/proj"),
        &reg,
        "",
    );
    // 抽出 "## Tool usage guidelines" 后所有 "- name:" 行
    let guidelines_section = prompt
        .split("\n## Tool usage guidelines\n")
        .nth(1)
        .unwrap_or("");
    let names: Vec<&str> = guidelines_section
        .lines()
        .filter(|l| l.starts_with("- "))
        .filter_map(|l| l.split(':').next())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(
        names, sorted,
        "guidelines must be sorted by tool name (deterministic token-cache)"
    );
}
