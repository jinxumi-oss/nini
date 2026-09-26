//! v0.8.3: Command result types — pure data, no logic.
//!
//! `CommandOutcome` describes what the dispatch should do (emit output,
//! quit, prompt for arg). `CommandResult` wraps an outcome with an
//! optional error message.

use super::registry::CommandId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    /// Side-effect happened; emit these lines into the transcript.
    Output(Vec<String>),
    /// User must exit the TUI.
    Quit,
    /// Switch the editor into a different mode (e.g., prompt for argument).
    PromptArgument { prompt: String, next: CommandId },
}

/// Result of running a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub outcome: CommandOutcome,
    /// Optional error message. When present, the runtime shows this to the user
    /// in the transcript instead of the normal output.
    pub error: Option<String>,
}

impl CommandResult {
    pub fn output(lines: Vec<String>) -> Self {
        Self {
            outcome: CommandOutcome::Output(lines),
            error: None,
        }
    }
    /// Wrap a command result that failed with an error message.
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            outcome: CommandOutcome::Output(vec![]),
            error: Some(msg.into()),
        }
    }
    pub fn quit() -> Self {
        Self {
            outcome: CommandOutcome::Quit,
            error: None,
        }
    }
    pub fn prompt(prompt: impl Into<String>, next: CommandId) -> Self {
        Self {
            outcome: CommandOutcome::PromptArgument {
                prompt: prompt.into(),
                next,
            },
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_wraps_lines() {
        let r = CommandResult::output(vec!["a".into(), "b".into()]);
        match r.outcome {
            CommandOutcome::Output(lines) => {
                assert_eq!(lines, vec!["a", "b"]);
            }
            _ => panic!("expected Output"),
        }
        assert!(r.error.is_none());
    }

    #[test]
    fn error_creates_output_with_error() {
        let r = CommandResult::error("oops");
        assert!(r.error.is_some());
        assert_eq!(r.error.as_deref(), Some("oops"));
    }

    #[test]
    fn quit_creates_quit_outcome() {
        let r = CommandResult::quit();
        assert!(matches!(r.outcome, CommandOutcome::Quit));
    }

    #[test]
    fn prompt_creates_prompt_outcome() {
        let r = CommandResult::prompt("enter arg", CommandId::Model);
        match r.outcome {
            CommandOutcome::PromptArgument { prompt, next } => {
                assert_eq!(prompt, "enter arg");
                assert_eq!(next, CommandId::Model);
            }
            _ => panic!("expected PromptArgument"),
        }
    }
}