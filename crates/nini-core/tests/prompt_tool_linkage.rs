//! 工具集 ↔ system prompt 的联动（v0.10.8 / T3）。
//!
//! 对照物：Pi 0.85.1 的 `dist/api/system-prompt.js`（快照在
//! `references/pi-0.85.1/dist/api/system-prompt.js`）。Pi 的三条规则：
//!
//! 1. 「Available tools」段**只列有 snippet 的工具**，一个都没有时写 `(none)`；
//! 2. guideline 由**已启用的工具集**决定：
//!    `if ((hasBash || hasPowerShell) && !hasGrep && !hasFind && !hasLs)`
//!    → "Use bash for file operations like ls, rg, find"；
//! 3. 永远有两条："Be concise in your responses" /
//!    "Show file paths clearly when working with files"。
//!
//! nini 此前是**自己发明的两段式**（"## Tool self-descriptions" +
//! "## Tool usage guidelines"），第 1、2 条都没有——也就是说
//! 「工具集变了，模型收到的引导就变了」这件事在 nini 上**不成立**。
//!
//! 这个文件是那道联动的 oracle：四种工具集组合 + 两条规则 + 一条不重复装配。

use async_trait::async_trait;
use nini_core::tool::{
    build_system_prompt_with_contributions, Tool, ToolContext, ToolError, ToolOutput, ToolRegistry,
    ToolSpec, ToolSystemPrompt,
};
use serde_json::{json, Value};
use std::sync::Arc;

/// 一个可控贡献的假工具（nini-core 不能依赖 nini-tools，否则成环）。
struct Fake {
    name: &'static str,
    snippet: Option<String>,
    guidelines: Vec<String>,
}

#[async_trait]
impl Tool for Fake {
    fn name(&self) -> &'static str {
        self.name
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.to_string(),
            description: format!("{} tool", self.name),
            input_schema: json!({"type": "object", "properties": {}}),
        }
    }
    async fn execute(&self, _args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            content: String::new(),
            is_error: false,
            details: None,
            duration_ms: 0,
            terminate: false,
        })
    }
    fn system_prompt_contribution(&self) -> Option<ToolSystemPrompt> {
        Some(ToolSystemPrompt {
            snippet: self.snippet.clone().unwrap_or_default(),
            guidelines: self.guidelines.clone(),
        })
    }
}

fn tool(name: &'static str, snippet: Option<&str>) -> Arc<dyn Tool> {
    Arc::new(Fake {
        name,
        snippet: snippet.map(|s| s.to_string()),
        guidelines: vec![],
    })
}

fn registry(tools: Vec<Arc<dyn Tool>>) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    for t in tools {
        reg.register_mut(t);
    }
    reg
}

fn prompt(reg: &ToolRegistry) -> String {
    build_system_prompt_with_contributions(Some("BASE"), reg).unwrap_or_default()
}

#[test]
fn a_tool_without_a_snippet_does_not_appear_in_available_tools() {
    // Pi 的规则 1：只有提供了一行简介的工具才进 "Available tools"。
    let reg = registry(vec![
        tool("bash", Some("Execute bash commands")),
        tool("mystery", None),
    ]);
    let p = prompt(&reg);
    assert!(
        p.contains("- bash: Execute bash commands"),
        "有 snippet 的工具必须列出：\n{p}"
    );
    assert!(
        !p.contains("mystery"),
        "没有 snippet 的工具不得出现在 Available tools 里：\n{p}"
    );
}

#[test]
fn an_empty_registry_says_none() {
    let reg = registry(vec![]);
    let p = prompt(&reg);
    assert!(
        p.contains("Available tools:\n(none)"),
        "一个工具都没有时必须写 (none)：\n{p}"
    );
}

#[test]
fn the_bash_guideline_appears_only_when_no_better_tool_exists() {
    // Pi 的规则 2：有 shell、但没有 grep/find/ls → 才让模型用 shell 做文件操作。
    let only_bash = registry(vec![tool("bash", Some("Execute bash commands"))]);
    let p = prompt(&only_bash);
    assert!(
        p.contains("- Use bash for file operations like ls, rg, find"),
        "只有 bash 时必须给出这条引导：\n{p}"
    );

    // 有了 find 之后，这条引导必须消失——否则模型会优先用 shell 去 ls，
    // 而 Pi 的行为正好相反（Pi 说「用 grep/find/ls」）。
    let with_find = registry(vec![
        tool("bash", Some("Execute bash commands")),
        tool("find", Some("Find files by glob pattern")),
    ]);
    let p2 = prompt(&with_find);
    assert!(
        !p2.contains("Use bash for file operations like ls, rg, find"),
        "有了 find 之后那条引导必须消失：\n{p2}"
    );
}

#[test]
fn the_two_always_on_guidelines_are_always_present() {
    // Pi 的规则 3
    for reg in [
        registry(vec![]),
        registry(vec![tool("read", Some("Read file contents"))]),
        registry(vec![tool("bash", Some("Execute bash commands"))]),
    ] {
        let p = prompt(&reg);
        assert!(
            p.contains("- Be concise in your responses"),
            "固定引导必须在：\n{p}"
        );
        assert!(
            p.contains("- Show file paths clearly when working with files"),
            "固定引导必须在：\n{p}"
        );
    }
}

#[test]
fn the_tool_sections_are_assembled_exactly_once() {
    // v0.10.8 (T3): 原来 CLI 的 prompt_setup 拼一次、agent 又拼一次，
    // 于是同一段工具说明在 prompt 里**出现两遍**。
    let reg = registry(vec![tool("bash", Some("Execute bash commands"))]);
    let p = prompt(&reg);
    let available = p.matches("Available tools:").count();
    let guidelines = p.matches("\nGuidelines:").count();
    assert_eq!(available, 1, "「Available tools」只能出现一次：\n{p}");
    assert_eq!(guidelines, 1, "「Guidelines」只能出现一次：\n{p}");
}
