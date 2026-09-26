//! v0.8.3: Slash command framework split into focused sub-modules.
//!
//! Structure (split for unit-testability):
//!   * `registry`     — CommandId enum, REGISTRY array, by_name, complete
//!   * `result`       — CommandOutcome, CommandResult
//!   * `parse`        — parse `/cmd args` → (CommandId, args)
//!   * `status_lines` — build_status_lines (for /status)
//!   * `html_export`  — render_transcript_html (for /export)
//!   * `dispatch`     — dispatch() + cmd_xxx helpers (28 commands)
//!
//! External API preserved via re-exports below.

pub mod dispatch;
pub mod html_export;
pub mod parse;
pub mod registry;
pub mod result;
pub mod status_lines;

pub use dispatch::dispatch;
pub use html_export::render_transcript_html;
pub use parse::parse;
pub use registry::{CommandDef, CommandId, REGISTRY, by_name, complete};
pub use result::{CommandOutcome, CommandResult};
pub use status_lines::build_status_lines;
