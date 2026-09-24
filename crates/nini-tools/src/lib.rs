//! nini-tools — built-in tool implementations (Bash, Read, Edit, Write, Grep, Find).

#![doc = "nini-tools — built-in tool implementations."]

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod bash;
pub mod bash_runner;
pub mod diff;
pub mod bash_executor;
pub mod edit;
pub mod find;
pub mod grep;
pub mod operations;
pub mod read;
pub mod write;

pub use bash::BashTool;
pub use edit::EditTool;
pub use find::FindTool;
pub use grep::GrepTool;
pub use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolRegistry, ToolSpec};
pub use read::ReadTool;
pub use write::WriteTool;
