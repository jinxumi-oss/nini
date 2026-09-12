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
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name as it appears in the model's tool schema.
    fn name(&self) -> &'static str;
    /// Static spec (description + JSON schema).
    fn spec(&self) -> ToolSpec;
    /// Execute the tool with the given (already-validated) arguments.
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError>;
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

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
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
