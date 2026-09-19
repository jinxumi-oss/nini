//! nini-ext — Pi-compatible extension host.
//!
//! Implements Rust trait-based extension API that mirrors pi's
//! `ExtensionAPI` (TypeScript). Extensions are dynamically-loaded Rust
//! shared libraries (cdylib) that implement the [`Extension`] trait and
//! expose functionality via [`ExtensionAPI`].
//!
//! ## v1 scope (covers 80% of pi extension use cases)
//! - `registerCommand(name, spec)` — slash-command registration
//! - `registerTool(tool)` — custom tools (LLM-callable)
//! - `sendUserMessage(text)` — push messages to the chat
//! - `getActiveModel()` — query current model
//! - `setStatus(text)` — update status bar
//! - `getCurrentCwd()` — query working directory
//!
//! ## Deferred to v2
//! - `registerShortcut` / `registerUIProvider` — requires UEFI-level UI
//! - `registerAutocompleteProvider` — needs async wiring
//! - `setEditorComponent` — full custom editor

use std::path::PathBuf;
use std::sync::Arc;

// Re-export core types extensions need.
pub use nini_core::provider::Provider;
pub use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolSpec};

/// Specification for a slash command registered by an extension.
/// Mirrors pi's `registerCommand` shape.
#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub description: String,
    pub argument_hint: Option<String>,
    /// Hidden commands don't appear in autocomplete.
    pub hidden: bool,
}

/// What an extension wants to do when its command is invoked.
pub type CommandHandler = Box<dyn Fn(&CommandContext) + Send + Sync>;

/// Context passed to a command handler. Mirrors pi's `ExtensionCommandContext`.
pub struct CommandContext {
    pub args: String,
    pub model: String,
    pub cwd: PathBuf,
}

/// Object-safe trait exposed to extensions via `register_*` methods.
/// Implemented by `RuntimeApi` (the side that talks back to nini).
pub trait ExtensionAPI: Send + Sync {
    /// Register a slash command (e.g., `/send-mail`) with a handler.
    fn register_command(&mut self, name: &str, spec: CommandSpec, handler: CommandHandler);

    /// Register a tool the LLM can call.
    fn register_tool(&mut self, tool: Arc<dyn Tool>);

    /// Push a user message into the chat (typically shown after the command runs).
    fn send_user_message(&self, text: &str);

    /// Get the currently-active model id.
    fn get_active_model(&self) -> String;

    /// Get the current working directory.
    fn get_current_cwd(&self) -> PathBuf;

    /// Update the TUI status bar with arbitrary text.
    fn set_status(&self, text: &str);
}

/// What an extension module exports. Implementations typically:
/// 1. Take `&mut dyn ExtensionAPI`
/// 2. Call `api.register_command(...)` / `api.register_tool(...)`
pub trait Extension: Send + Sync {
    /// Called once at load. Extension should register everything here.
    fn activate(&self, api: &mut dyn ExtensionAPI);
}

/// Loadable extension metadata.
#[derive(Debug, Clone)]
pub struct ExtensionInfo {
    pub name: String,
    pub path: PathBuf,
    pub version: String,
}

pub mod loader;
pub mod runtime_api;

pub use loader::ExtensionLoader;
pub use runtime_api::RuntimeApi;

/// Library version, mirrors workspace version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
