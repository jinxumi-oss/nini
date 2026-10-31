//! v0.15: `WrappedTool` must forward the inner tool's execution mode.
//!
//! `WrappedTool` is the only construction point of the trust-gate
//! wrapper (`crates/nini-core/src/trust_gate.rs`), and the trust gate
//! wraps **every** tool the project registers. The wrapper forwards
//! `name()` and `spec()` to the inner tool, but it did not forward
//! `execution_mode()` — so a tool that declared itself `Sequential`
//! lost that declaration the moment it was wrapped, and the agent loop
//! (`agent.rs`, `t.execution_mode()`) saw `Parallel` instead.
//!
//! The consequence is not subtle: a tool that must run alone (it owns
//! exclusive state) runs concurrently with its siblings after wrapping.
//!
//! The mode a tool declares must survive being wrapped — before and
//! after.

use async_trait::async_trait;
use nini_core::tool::{
    Tool, ToolContext, ToolError, ToolExecutionMode, ToolOutput, ToolSpec, WrappedTool,
};
use serde_json::{json, Value};
use std::sync::Arc;

/// A tool that declares its own execution mode.
struct ModeTool {
    mode: ToolExecutionMode,
}

#[async_trait]
impl Tool for ModeTool {
    fn name(&self) -> &'static str {
        "mode-tool"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "mode-tool".to_string(),
            description: "Declares an execution mode (test only).".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
        }
    }

    async fn execute(&self, _args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::ok("ok"))
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        self.mode
    }
}

/// A tool that declared itself `Sequential` must still be `Sequential`
/// after being wrapped.
#[test]
fn a_sequential_tool_stays_sequential_after_wrapping() {
    let inner: Arc<dyn Tool> = Arc::new(ModeTool {
        mode: ToolExecutionMode::Sequential,
    });
    assert_eq!(
        inner.execution_mode(),
        ToolExecutionMode::Sequential,
        "the fixture is wrong: the inner tool is not sequential"
    );

    let wrapped = WrappedTool::new(inner, None, None);
    assert_eq!(
        wrapped.execution_mode(),
        ToolExecutionMode::Sequential,
        "the wrapper swallowed the tool's execution mode"
    );
}

/// The other direction, so the assertion above cannot be a tautology:
/// a `Parallel` tool must still be `Parallel` after being wrapped.
#[test]
fn a_parallel_tool_stays_parallel_after_wrapping() {
    let inner: Arc<dyn Tool> = Arc::new(ModeTool {
        mode: ToolExecutionMode::Parallel,
    });
    assert_eq!(
        inner.execution_mode(),
        ToolExecutionMode::Parallel,
        "the fixture is wrong: the inner tool is not parallel"
    );

    let wrapped = WrappedTool::new(inner, None, None);
    assert_eq!(
        wrapped.execution_mode(),
        ToolExecutionMode::Parallel,
        "the wrapper invented an execution mode the tool did not declare"
    );
}
