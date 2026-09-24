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

pub mod ansi;

pub mod clipboard;
pub mod commands;
pub mod command_palette;
pub mod editor;
pub mod event_bus;
pub mod file_completion;
pub mod help_overlay;
pub mod hyperlink;
pub mod image_paste;
pub mod keybindings_manager;
pub mod keys;
pub mod markdown;
pub mod render;
pub mod rich;
pub mod runtime;
pub mod selector;
pub mod selectors;
pub mod settings;
pub mod signals;
pub mod state;
pub mod theme;
pub mod theme_watcher;

pub use commands::{
    CommandId, CommandOutcome, CommandResult, REGISTRY, by_name, complete, dispatch, parse,
};
pub use keybindings_manager::KeybindingsManager;
pub use keys::{Key, KeyAction, KeyBinding, KeyModifiers, default_keymap, resolve};
pub use runtime::run;
pub use state::{AppState, InputBuffer, RunMode, TokenStats, TranscriptLine};
pub use theme::{COLOR_NAMES, Theme};
