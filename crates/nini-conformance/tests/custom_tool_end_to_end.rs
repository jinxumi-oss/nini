//! X1：自定义工具在 `apply_project_trust` 之后**端到端**仍然完整。
//!
//! 需求背景（`docs/plans/surpass-pi-optimization-plan.md` X1）：nini 的
//! `ToolRegistry` 已有 `register`/`register_mut`，`trust_gate` 已能包裹每个
//! 工具，但「自定义工具是否真的进入系统提示词的工具列表、snippet、
//! guidelines、信任门控、可被调用」需要端到端验证——断了就是「声明了扩展点
//! 但用户不可见」的静默缺陷。
//!
//! **这个文件抓住一个真实缺陷**：`WrappedTool` 没有转发
//! `system_prompt_contribution()`（继承默认 `None`）。于是
//! `apply_project_trust` 把每个工具都包成 `WrappedTool` 之后，**任何自定义
//! 工具的 snippet/guidelines 都从系统提示词里消失了**——模型再也看不到它的
//! 用法说明，而注册表里它还在。这是「声明了却静默丢弃」，正是 nini 自己
//! 反对的那类失真。
//!
//! 三条断言覆盖三个面：
//! 1. 包裹后，自定义工具的 snippet/guidelines 仍在系统提示词里；
//! 2. 包裹后，自定义工具仍被信任门控覆盖（`before()` 非空）；
//! 3. 包裹后，自定义工具仍能被调用（execute 执行，结果返回）。

use async_trait::async_trait;
use nini_core::project_trust::ProjectTrustStore;
use nini_core::tool::{
    build_system_prompt_with_contributions, Tool, ToolContext, ToolError, ToolOutput, ToolRegistry,
    ToolSpec, ToolSystemPrompt,
};
use nini_core::trust_gate::apply_project_trust;
use serde_json::{json, Value};
use std::sync::Arc;

/// 一个可控的自定义工具：带 snippet + guideline，execute 记录自己被调用。
struct Custom {
    name: &'static str,
    snippet: String,
    guidelines: Vec<String>,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl Tool for Custom {
    fn name(&self) -> &'static str {
        self.name
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.to_string(),
            description: format!("{} custom tool", self.name),
            input_schema: json!({"type": "object", "properties": {}}),
        }
    }
    async fn execute(&self, _args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(ToolOutput {
            content: "custom ran".to_string(),
            is_error: false,
            details: None,
            duration_ms: 0,
            terminate: false,
        })
    }
    fn system_prompt_contribution(&self) -> Option<ToolSystemPrompt> {
        Some(ToolSystemPrompt {
            snippet: self.snippet.clone(),
            guidelines: self.guidelines.clone(),
        })
    }
}

/// 用一个信任商店把 `apply_project_trust` 的包裹真正启用（有 store 才会包）。
fn gated_registry(name: &'static str) -> (ToolRegistry, Arc<Custom>) {
    let custom = Arc::new(Custom {
        name,
        snippet: format!("{name} runs bespoke work"),
        guidelines: vec![format!("call {name} for bespoke work")],
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let mut reg = ToolRegistry::new();
    reg.register_mut(custom.clone());
    // 指向一个带 trust.json 的临时 agent 目录，让 `load_store` 返回 Some，
    // 于是每个工具被 `WrappedTool` 包裹。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trust.json");
    ProjectTrustStore::default().save(&path).unwrap();
    // `apply_project_trust` 通过 `paths::agent_file` 找 trust.json，它读
    // `NINI_AGENT_DIR`。这里把它指向上面的临时目录。
    let saved = std::env::var_os("NINI_AGENT_DIR");
    // `env_lock` 与其它测试共用同一把锁，保证环境改动串行。
    let _guard = nini_core::paths::env_lock();
    unsafe { std::env::set_var("NINI_AGENT_DIR", dir.path()) };
    let wrapped = apply_project_trust(reg, std::path::Path::new("/tmp/proj"));
    match saved {
        Some(v) => unsafe { std::env::set_var("NINI_AGENT_DIR", v) },
        None => unsafe { std::env::remove_var("NINI_AGENT_DIR") },
    }
    (wrapped, custom)
}

fn prompt(reg: &ToolRegistry) -> String {
    build_system_prompt_with_contributions(Some("BASE"), reg).unwrap_or_default()
}

/// 1. 包裹后，自定义工具的 snippet 与 guideline 仍在系统提示词里。
///
/// 变异注入：若 `WrappedTool` 不转发 `system_prompt_contribution()`（当前
/// 的真实缺陷），这个测试会红——snippet 段为空，提示词里没有该工具名。
#[test]
fn a_custom_tool_keeps_its_prompt_contribution_after_trust_wrapping() {
    let (reg, _custom) = gated_registry("my-custom");
    let p = prompt(&reg);

    assert!(
        p.contains("Available tools:"),
        "the Available-tools section must exist:\n{p}"
    );
    assert!(
        p.contains("my-custom"),
        "the custom tool's name must survive trust wrapping (its snippet must \
         still reach the prompt). Prompt:\n{p}"
    );
    assert!(
        p.contains("my-custom runs bespoke work"),
        "the custom tool's snippet must survive trust wrapping:\n{p}"
    );
    assert!(
        p.contains("call my-custom for bespoke work"),
        "the custom tool's guideline must survive trust wrapping:\n{p}"
    );
}

/// 2. 包裹后，自定义工具仍被信任门控覆盖（`before()` 非空）。
#[test]
fn a_custom_tool_is_still_gated_after_trust_wrapping() {
    let (reg, _custom) = gated_registry("my-gated");
    let tool = reg
        .get("my-gated")
        .expect("the custom tool must still be registered after wrapping");
    assert!(
        tool.before().is_some(),
        "the custom tool must still carry the trust gate after wrapping"
    );
}

/// 3. 包裹后，自定义工具仍能被调用。
#[tokio::test]
async fn a_custom_tool_still_executes_after_trust_wrapping() {
    let (reg, custom) = gated_registry("my-exec");
    let tool = reg.get("my-exec").expect("registered after wrapping");

    // 信任目录是允许的（`default()` 的 `Ask` → allow），execute 应成功执行。
    let out = tool
        .execute(json!({}), ToolContext::default())
        .await
        .expect("the custom tool must execute through the wrapper");
    assert!(!out.is_error);
    assert_eq!(out.content, "custom ran");
    assert_eq!(
        custom.calls.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "the inner tool's execute must actually run"
    );
}
