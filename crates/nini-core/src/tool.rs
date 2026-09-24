//! Tool trait, spec, output, context, error, and registry.
//!
//! Concrete tool impls (Bash, Read, etc.) live in `nini-tools`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

/// Static specification of a tool (name + description + JSON schema).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Result of a tool invocation, returned to the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Text content (already-truncated if necessary).
    pub content: String,
    /// True if the tool errored (network, exit non-zero, etc.).
    pub is_error: bool,
    /// Optional structured details (exit code, duration, etc.).
    pub details: Option<Value>,
}

impl ToolOutput {
    /// Convenience: a successful text output.
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            details: None,
        }
    }

    /// Convenience: an error output.
    pub fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            details: None,
        }
    }
}

/// Context passed to a tool on each invocation.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Working directory for the tool (typically the session's cwd).
    pub cwd: std::path::PathBuf,
    /// Token output budget (1 MiB by default; tool should truncate when exceeded).
    pub max_output_bytes: usize,
    /// Token line budget (2000 lines by default).
    pub max_output_lines: usize,
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            max_output_bytes: 1_048_576, // 1 MiB
            max_output_lines: 2000,
        }
    }
}

/// Tool error type.
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("execution timed out after {0} ms")]
    Timeout(u64),
    #[error("tool not found: {0}")]
    NotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("internal: {0}")]
    Internal(String),
}

impl From<std::io::Error> for ToolError {
    fn from(e: std::io::Error) -> Self {
        ToolError::Io(e.to_string())
    }
}

/// The `Tool` trait: implementors provide a name, spec, and async execute.
///
/// v0.7 (M2) — extends the trait with optional `before()` / `after()`
/// hook slots so extensions can intercept tool execution without
/// rewriting the tool itself. Both default to `None`, which means
/// "no hook installed" = v0.6.x behavior. Existing tool impls are
/// unaffected.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name as it appears in the model's tool schema.
    fn name(&self) -> &'static str;
    /// Static spec (description + JSON schema).
    fn spec(&self) -> ToolSpec;
    /// Execute the tool with the given (already-validated) arguments.
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError>;

    /// v0.7 (M2) — return a `BeforeExecute` hook to intercept the
    /// tool call BEFORE `execute()` runs. Default: no hook.
    ///
    /// The hook receives the parsed `Value` and the `ToolContext`
    /// and may:
    ///   * Return `Ok(Some(modified_args))` to substitute new args
    ///   * Return `Ok(None)` to pass through unchanged
    ///   * Return `Err(...)` to deny execution with that error
    ///     message (the agent surfaces it as a `ToolResult` with
    ///     `is_error: true`).
    ///
    /// Common use cases: permission checks (TrustStore), path
    /// sanitization, arg rewriting, audit logging.
    fn before(&self) -> Option<Box<dyn BeforeExecute>> {
        None
    }

    /// v0.7 (M2) — return an `AfterExecute` hook to intercept the
    /// tool output AFTER `execute()` succeeds. Default: no hook.
    ///
    /// The hook receives the raw `ToolOutput` and may rewrite
    /// `content` (e.g. for redaction), flip `is_error`, or attach
    /// extra `details`.
    ///
    /// NOTE: hooks only fire on the success path. A tool that
    /// returns `Err` from `execute()` is surfaced as-is.
    fn after(&self) -> Option<Box<dyn AfterExecute>> {
        None
    }
}

/// v0.7 (M2) hook trait — runs BEFORE `Tool::execute()`.
///
/// Implementors may return modified args, deny the call, or pass
/// through unchanged. See `Tool::before()` for the contract.
#[async_trait]
pub trait BeforeExecute: Send + Sync {
    async fn run(
        &self,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<Option<Value>, ToolError>;
}

/// v0.7 (M2) hook trait — runs AFTER `Tool::execute()` succeeds.
///
/// Implementors may rewrite the output. See `Tool::after()` for the
/// contract.
#[async_trait]
pub trait AfterExecute: Send + Sync {
    async fn run(&self, output: ToolOutput, ctx: &ToolContext)
        -> Result<ToolOutput, ToolError>;
}

/// Registry of tools, indexed by name.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool. Replaces any existing tool with the same name.
    pub fn register(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.insert(tool.name().to_string(), tool);
        self
    }

    /// In-place registration. Useful when the registry is mutated inside a
    /// loop where chaining would consume the receiver.
    pub fn register_mut(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Iterate over all registered tools (as `Arc<dyn Tool>`).
    pub fn tools(&self) -> impl Iterator<Item = Arc<dyn Tool>> + '_ {
        self.tools.values().cloned()
    }

    /// List all registered tool specs (for the model).
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|t| t.spec()).collect()
    }

    /// List all registered tool names.
    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }
}

/// v0.7 (M2) — `WrappedTool` lets extensions inject before/after
/// hooks around an existing tool without rewriting the tool.
///
/// The wrapper forwards `name()` and `spec()` to the inner tool and
/// only adds the hook slots. The agent loop still drives
/// `execute()` directly on the inner tool, but the wrapper exposes
/// `before()` / `after()` so the run loop can call the hooks around
/// the execute.
///
/// Common pattern: wrap an existing Read tool with a path whitelist
/// `BeforeExecute` to block reads outside the project root.
///
/// Hooks are stored as `Arc<dyn ...>` so the `before()` /
/// `after()` methods can return a fresh `Box<dyn ...>` clone on
/// every call (the trait return type owns the hook).
pub struct WrappedTool {
    inner: Arc<dyn Tool>,
    before_hook: Option<Arc<dyn BeforeExecute>>,
    after_hook: Option<Arc<dyn AfterExecute>>,
}

impl WrappedTool {
    /// Wrap `inner` with optional hooks. Pass `None` for slots you
    /// don't want to override.
    pub fn new(
        inner: Arc<dyn Tool>,
        before_hook: Option<Arc<dyn BeforeExecute>>,
        after_hook: Option<Arc<dyn AfterExecute>>,
    ) -> Self {
        Self {
            inner,
            before_hook,
            after_hook,
        }
    }

    /// Borrow the inner tool (e.g. to call `execute()` directly).
    pub fn inner(&self) -> &Arc<dyn Tool> {
        &self.inner
    }
}

#[async_trait]
impl Tool for WrappedTool {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn spec(&self) -> ToolSpec {
        self.inner.spec()
    }
    async fn execute(
        &self,
        args: Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        self.inner.execute(args, ctx).await
    }
    fn before(&self) -> Option<Box<dyn BeforeExecute>> {
        // Return a thin adapter that clones the Arc so the call
        // site gets an owned Box<dyn BeforeExecute + 'static>.
        let hook = self.before_hook.as_ref()?.clone();
        let boxed: Box<dyn BeforeExecute> = Box::new(ArcBoxAdapter(hook));
        Some(boxed)
    }
    fn after(&self) -> Option<Box<dyn AfterExecute>> {
        let hook = self.after_hook.as_ref()?.clone();
        let boxed: Box<dyn AfterExecute> = Box::new(ArcBoxAdapterAfter(hook));
        Some(boxed)
    }
}

/// Adapter from `Arc<dyn BeforeExecute>` to `Box<dyn BeforeExecute>`.
/// Wrapping the Arc in a newtype lets us box it on each call without
/// the lifetime problem that a raw `&dyn` adapter would have.
struct ArcBoxAdapter(Arc<dyn BeforeExecute>);
#[async_trait]
impl BeforeExecute for ArcBoxAdapter {
    async fn run(
        &self,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<Option<Value>, ToolError> {
        self.0.run(args, ctx).await
    }
}

/// Public re-export of the Arc adapter so test code (and any
/// downstream extension) can construct `Box<dyn BeforeExecute>` from
/// an `Arc<dyn BeforeExecute>` without rolling their own adapter.
pub struct ArcBoxAdapterBefore(pub Arc<dyn BeforeExecute>);
#[async_trait]
impl BeforeExecute for ArcBoxAdapterBefore {
    async fn run(
        &self,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<Option<Value>, ToolError> {
        self.0.run(args, ctx).await
    }
}

struct ArcBoxAdapterAfter(Arc<dyn AfterExecute>);
#[async_trait]
impl AfterExecute for ArcBoxAdapterAfter {
    async fn run(
        &self,
        output: ToolOutput,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        self.0.run(output, ctx).await
    }
}

/// Public re-export of the Arc adapter for `AfterExecute`.
pub struct ArcBoxAdapterAfterTool(pub Arc<dyn AfterExecute>);
#[async_trait]
impl AfterExecute for ArcBoxAdapterAfterTool {
    async fn run(
        &self,
        output: ToolOutput,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        self.0.run(output, ctx).await
    }
}
