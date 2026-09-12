//! nini-tools — built-in tool implementations (Bash, Read, Edit, Write, Grep, Find).

#![doc = "nini-tools — built-in tool implementations."]

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod bash;
pub mod read;
pub mod write;
pub mod edit;
pub mod grep;
pub mod find;

pub use bash::BashTool;
pub use nini_core::tool::{ Tool, ToolContext, ToolError, ToolOutput, ToolRegistry, ToolSpec };
pub use read::ReadTool;
pub use write::WriteTool;
pub use edit::EditTool;
pub use grep::GrepTool;
pub use find::FindTool;