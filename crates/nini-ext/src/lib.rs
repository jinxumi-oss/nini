//! nini-ext — Pi-compatible extension host.
//!
//! # Status: NOT WIRED, and the approach is wrong (v0.10, M3)
//!
//! **Nothing depends on this crate.** No crate in the workspace lists
//! `nini-ext` as a dependency and no production file references it. It
//! compiles as a workspace member and that is the whole of its
//! participation.
//!
//! ## The finding
//!
//! The plan recorded this as "implemented but never wired — 491 lines,
//! zero call sites, connect it and it works" (`docs/plans/
//! v0.10-pi-parity-refactor.md` §5). Reading Pi's source says otherwise:
//! **wiring it would not work**, and that is a different problem with a
//! different answer.
//!
//! | | Pi's extensions | this crate |
//! |---|---|---|
//! | unit | a `.ts` / `.js` file | a compiled Rust `cdylib` |
//! | loaded by | `jiti` (`core/extensions/loader.ts:17`) | `libloading::Library::new` + the C ABI symbol `nini_ext_activate` |
//! | surface | 28 runtime events on `ExtensionAPI`, plus `import` of Pi's own packages (`loader.ts:27`) | `trait ExtensionAPI` with 6 methods |
//!
//! A Pi extension is TypeScript evaluated in a JS runtime. `dlopen` cannot
//! load one, and `dlopen` is the only thing this loader does. There is no
//! overlap: not a missing call site, not a missing event, a different
//! execution model.
//!
//! Making it compatible would mean embedding a JS engine **and** shipping
//! a compatibility shim for `ExtensionAPI`'s 28 events **and** resolving
//! `import ... from "@earendil-works/pi-coding-agent"` inside a Rust
//! binary. That is a project, not a wiring change.
//!
//! ## Why it has not been deleted
//!
//! Two reasons, and the second is the one that matters.
//!
//! 1. Deletion is irreversible, and v0.8.4 and v0.9.0 both deleted
//!    something that turned out to be a regression and had to be restored.
//!    The plan's own rule is "do not delete unwired Pi architecture".
//! 2. That rule's premise does not currently hold - this is not an
//!    unfinished refactor, it is a refactor toward the wrong target. But
//!    "the plan's rule does not apply" is a claim that deserves evidence,
//!    and the evidence is one paragraph long and written on 2026-10-12.
//!    Reversible now, cheaper than a second restore.
//!
//! ## What replaces it
//!
//! Ecosystem access does not have to go through this crate. `SKILL.md`
//! is already read from Pi's and Claude Code's directories (see
//! `nini_core::skills::skill_roots`) with no dependency on either agent.
//! For *tools*, the cross-ecosystem answer is a process protocol, not a
//! dynamic-linker ABI - see the plan's P2.
//!
//! ## When this gets deleted
//!
//! When the extension runtime is implemented in whatever form the evidence
//! supports, this crate has no referent. If that form is an embedded JS
//! runtime, it is a new crate and this one goes. If the ecosystem turns
//! out to be reachable over MCP alone, this goes then too.
//!
//! Until then it stays, unwired, with this comment, so that the next
//! reader does not mistake "implemented" for "planned".
//!
//! ## Original scope notes (kept for the record)
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
