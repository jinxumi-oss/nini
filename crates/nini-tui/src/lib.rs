//! nini-tui — terminal UI for nini.
//!
//! Phase 5: ratatui + crossterm interactive TUI.
//!
//! Architecture:
//! - [`keys`]: pure key parsing (`crossterm::KeyEvent` → `KeyAction`)
//! - [`state`]: pure app state (`AppState`, `InputBuffer`, `TranscriptLine`)
//! - [`render`]: pure `AppState → ratatui::Frame` rendering
//! - [`runtime`]: terminal setup + async event loop
//!
//! All non-IO layers are unit-testable without a real terminal.

#![doc = "nini-tui — interactive terminal UI for nini."]

/// Library version, mirrors workspace version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod keys;
pub mod state;
pub mod render;
pub mod runtime;
pub mod commands;

pub use keys::{ default_keymap, resolve, Key, KeyAction, KeyBinding, KeyModifiers };
pub use runtime::run;
pub use state::{ AppState, InputBuffer, RunMode, TokenStats, TranscriptLine };
pub use commands::{ CommandId, REGISTRY, by_name, complete, dispatch, parse, CommandOutcome, CommandResult };