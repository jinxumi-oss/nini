//! TUI runtime: alternate screen + raw mode + event loop + agent integration.
//!
//! Two main pieces:
//! - [`run_loop`]: drives the terminal in async mode, handles keystrokes
//!   and triggers redraws.
//! - [`AgentTask`]: spawned on Submit, runs the agent and pushes events
//!   into the shared state via [`AgentSink`].
#![allow(unused_mut)] // render/runtime use mut bindings for future hook points

use crate::keys::{Key, KeyAction, resolve};
use crate::render::render_frame;
use crate::state::{AppState, RunMode, TranscriptLine};
use anyhow::Result;
use crossterm::event::KeyCode;
use crossterm::event::{Event, EventStream, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::{Stdout, stdout};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// Handle to the live TUI state, shared between the runtime loop and the
/// agent task. The runtime owns the state; the agent task borrows it.
pub type SharedState = Arc<Mutex<AppState>>;

/// Builder: `SharedState::new(state)` produces an `Arc<Mutex<AppState>>`.
pub fn shared_state(state: AppState) -> SharedState {
    Arc::new(Mutex::new(state))
}

/// A trimmed-down agent event for cross-task delivery. Avoids leaking
/// `nini_core::AgentEvent` (and its `AgentError`/`Provider` dependencies)
/// across the spawn boundary.
#[derive(Debug, Clone)]
pub enum AgentEventLite {
    TextDelta(String),
    ToolCallStart {
        name: String,
    },
    ToolCallStop {
        id: String,
        args: String,
    },
    ToolResult {
        ok: bool,
        content: String,
    },
    TurnEnd,
    Error(String),
    Usage(u32, u32),
    /// Marks end of agent run; flips state back to Editing.
    Done,
}

/// Cloneable handle for pushing agent events into the TUI state from a
/// background tokio task.
#[derive(Clone)]
pub struct AgentSink {
    state: SharedState,
}

impl AgentSink {
    pub fn new(state: SharedState) -> Self {
        Self { state }
    }

    /// Apply an `AgentEventLite` to the state.
    pub fn push(&self, ev: AgentEventLite) {
        if let Ok(mut s) = self.state.lock() {
            match ev {
                AgentEventLite::TextDelta(text) => s.push_assistant(text),
                AgentEventLite::ToolCallStart { name } => s.push_tool_call(name, ""),
                AgentEventLite::ToolCallStop { id, args } => {
                    // Update the most recent tool call line with final args.
                    if let Some(TranscriptLine::ToolCall { args: a, .. }) = s.transcript.last_mut()
                    {
                        *a = args;
                    } else {
                        s.push_tool_call(id, args);
                    }
                }
                AgentEventLite::ToolResult { ok, content } => {
                    s.push_tool_result(ok, content);
                }
                AgentEventLite::TurnEnd => s.push_divider(),
                AgentEventLite::Error(message) => {
                    s.push_assistant(format!("[error] {message}"));
                }
                AgentEventLite::Usage(input, output) => {
                    s.tokens.input += input as u64;
                    s.tokens.output += output as u64;
                }
                AgentEventLite::Done => {
                    s.mode = RunMode::Editing;
                    s.status = "ready".to_string();
                }
            }
        }
    }
}

/// A function that drives one agent turn. Called by the runtime after Submit.
/// `sink` receives the agent's events. `done` is notified when the turn
/// finishes (whether successfully, with error, or aborted).
pub type AgentDriver = Arc<dyn Fn(String, AgentSink, Arc<Notify>) -> JoinHandle<()> + Send + Sync>;

/// Public entrypoint: run the TUI. `bootstrap` is called once to seed the
/// `AppState`. `agent_driver` is spawned whenever the user submits a message.
pub async fn run<F>(bootstrap: F, agent_driver: AgentDriver) -> Result<()>
where
    F: FnOnce(&mut AppState),
{
    let mut state = AppState::new("test-model");
    bootstrap(&mut state);
    let shared = shared_state(state);

    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    terminal.hide_cursor()?;

    let result = run_loop(&mut terminal, shared, agent_driver).await;

    terminal.show_cursor()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    disable_raw_mode()?;
    result
}

/// Main event loop. Returns when the user quits.
async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    shared: SharedState,
    agent_driver: AgentDriver,
) -> Result<()> {
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tick.tick().await; // skip the immediate first tick

    loop {
        // Snapshot state for rendering (cheap clone, doesn't hold lock long).
        let snapshot = {
            let g = shared.lock().unwrap();
            g.clone()
        };
        terminal.draw(|f| render_frame(f, &snapshot))?;

        let done = Arc::new(Notify::new()); // per-iteration done signal
        let done_for_select = done.clone();

        tokio::select! {
            maybe = events.next() => {
                match maybe {
                    Some(Ok(Event::Key(k))) => handle_key(k, &shared, &agent_driver, done),
                    Some(Ok(Event::Resize(_, _))) => { /* ratatui handles */ }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => eprintln!("event error: {e}"),
                    None => break,
                }
            }
            _ = done_for_select.notified() => {
                // Agent finished; loop will redraw on next iteration.
            }
            _ = tick.tick() => { /* animation tick */ }
        }

        let mode = shared.lock().unwrap().mode;
        if mode == RunMode::Quitting {
            return Ok(());
        }
    }
    Ok(())
}

/// Map a `crossterm::KeyEvent` to a `KeyAction` and apply to state.
/// On Submit (in Editing mode), spawn the agent runner.
fn handle_key(k: KeyEvent, shared: &SharedState, agent_driver: &AgentDriver, done: Arc<Notify>) {
    let mut state = shared.lock().unwrap();
    let key: Key = k.into();

    // Tab is not in default_keymap; intercept it explicitly for completion.
    if matches!(k.code, KeyCode::Tab) {
        if state.completion.is_some() {
            state.apply_completion();
        } else if state.mode == RunMode::Editing {
            // No popup: Tab inserts a literal tab character.
            state.input.insert_char('\t');
        }
        return;
    }

    let action = resolve(&crate::keys::default_keymap(), key);
    // Drop the lock while we hold it; the rest of the match needs it.

    match action {
        KeyAction::Insert(c) => {
            if state.mode == RunMode::Editing {
                state.input.insert_char(c);
                state.refresh_completion();
            }
        }
        KeyAction::Newline => {
            if state.mode == RunMode::Editing {
                state.input.insert_char('\n');
                state.refresh_completion();
            }
        }
        KeyAction::Backspace => {
            state.input.backspace();
            state.refresh_completion();
        }
        KeyAction::Delete => {
            state.input.delete();
            state.refresh_completion();
        }
        KeyAction::MoveLeft => {
            if state.completion.is_some() {
                state.completion.as_mut().unwrap().select_up();
            } else {
                state.input.move_left();
            }
        }
        KeyAction::MoveRight => {
            if state.completion.is_some() {
                state.completion.as_mut().unwrap().select_down();
            } else {
                state.input.move_right();
            }
        }
        KeyAction::MoveLineStart => state.input.move_to_start(),
        KeyAction::MoveLineEnd => state.input.move_to_end(),
        KeyAction::MoveWordLeft => state.input.move_word_left(),
        KeyAction::MoveWordRight => state.input.move_word_right(),
        KeyAction::MoveUp => {
            if state.completion.is_some() {
                state.completion.as_mut().unwrap().select_up();
            } else {
                state.input.recall_history(-1);
            }
        }
        KeyAction::MoveDown => {
            if state.completion.is_some() {
                state.completion.as_mut().unwrap().select_down();
            } else {
                state.input.recall_history(1);
            }
        }
        KeyAction::KillToLineStart => state.input.kill_to_line_start(),
        KeyAction::KillToLineEnd => state.input.kill_to_line_end(),
        KeyAction::KillWordBackward => state.input.kill_word_backward(),
        KeyAction::ClearInput => state.input.clear(),
        KeyAction::Submit => {
            if state.completion.is_some() {
                state.apply_completion();
            } else {
                drop(state);
                submit_user_input(shared, agent_driver, done);
            }
        }
        KeyAction::Abort => {
            if state.completion.is_some() {
                state.completion = None;
            } else if state.mode == RunMode::Running {
                state.mode = RunMode::Aborted;
                state.status = "aborted".to_string();
            } else {
                state.input.clear();
            }
        }
        KeyAction::Quit => state.mode = RunMode::Quitting,
        KeyAction::SwitchModel => {
            state.status = "switch model (not yet implemented)".to_string();
        }
        KeyAction::ShowHelp => {
            state.push_divider();
            state.push_assistant(
                "F1=help  Ctrl+C=quit  Ctrl+D=exit  Enter=send  Ctrl+L=model  ↑↓=history",
            );
            state.push_divider();
        }
        KeyAction::ScrollUp | KeyAction::ScrollDown | KeyAction::Noop => {}
    }
}

/// Submit handler: extract text, transition to Running, spawn agent task.
pub fn submit_user_input(shared: &SharedState, agent_driver: &AgentDriver, done: Arc<Notify>) {
    // Lock, mutate, snapshot, drop.
    let text = {
        let mut g = shared.lock().unwrap();
        if g.mode != RunMode::Editing {
            return;
        }
        let text = g.input.submit();
        if text.trim().is_empty() {
            return;
        }
        g.push_user(text.clone());
        g.push_divider();
        g.mode = RunMode::Running;
        g.status = "running...".to_string();
        text
    };

    // Spawn the agent task with its own sink.
    let sink = AgentSink::new(shared.clone());
    let _handle = (agent_driver)(text, sink, done.clone());
    // The handle is intentionally dropped — the task continues running in
    // the background. We don't abort the agent on quit; the runtime owns
    // the SharedState and the sink keeps a clone.
}

/// Apply a `KeyAction` to a state without spawning anything. Used by tests
/// and the no-agent mode.
pub fn apply_action(state: &mut AppState, key: Key) {
    let action = resolve(&crate::keys::default_keymap(), key);
    match action {
        KeyAction::Insert(c) => {
            if state.mode == RunMode::Editing {
                state.input.insert_char(c);
                state.refresh_completion();
            }
        }
        KeyAction::Newline => {
            if state.mode == RunMode::Editing {
                state.input.insert_char('\n');
                state.refresh_completion();
            }
        }
        KeyAction::Backspace => {
            state.input.backspace();
            state.refresh_completion();
        }
        KeyAction::Delete => {
            state.input.delete();
            state.refresh_completion();
        }
        KeyAction::MoveLeft => {
            // Popup navigation: if popup visible and cursor is at start,
            // arrow up should select previous item. Otherwise it's
            // standard cursor motion.
            if state.completion.is_some() {
                if state.completion.as_ref().unwrap().selected == 0 {
                    // wrap
                } else {
                    state.completion.as_mut().unwrap().select_up();
                }
            } else {
                state.input.move_left();
            }
        }
        KeyAction::MoveRight => {
            if state.completion.is_some() {
                state.completion.as_mut().unwrap().select_down();
            } else {
                state.input.move_right();
            }
        }
        KeyAction::MoveLineStart => state.input.move_to_start(),
        KeyAction::MoveLineEnd => state.input.move_to_end(),
        KeyAction::MoveWordLeft => state.input.move_word_left(),
        KeyAction::MoveWordRight => state.input.move_word_right(),
        KeyAction::MoveUp => {
            if state.completion.is_some() {
                state.completion.as_mut().unwrap().select_up();
            } else {
                state.input.recall_history(-1);
            }
        }
        KeyAction::MoveDown => {
            if state.completion.is_some() {
                state.completion.as_mut().unwrap().select_down();
            } else {
                state.input.recall_history(1);
            }
        }
        KeyAction::KillToLineStart => state.input.kill_to_line_start(),
        KeyAction::KillToLineEnd => state.input.kill_to_line_end(),
        KeyAction::KillWordBackward => state.input.kill_word_backward(),
        KeyAction::ClearInput => state.input.clear(),
        KeyAction::Submit => {
            if state.mode == RunMode::Editing {
                // If completion popup is showing, accept the selected item
                // instead of submitting.
                if state.completion.is_some() {
                    state.apply_completion();
                    return;
                }
                let text = state.input.submit();
                if !text.trim().is_empty() {
                    // Slash command interception
                    if let Some((cmd_id, args)) = crate::commands::parse(&text) {
                        use crate::commands::dispatch;
                        let result = dispatch(state, cmd_id, &args);
                        match result.outcome {
                            crate::commands::CommandOutcome::Output(lines) => {
                                for line in lines {
                                    state.push_assistant(line);
                                }
                                state.push_divider();
                            }
                            crate::commands::CommandOutcome::Quit => {
                                state.mode = RunMode::Quitting;
                            }
                            crate::commands::CommandOutcome::PromptArgument { .. } => {
                                // v1: prompt-argument flow not implemented; show fallback.
                                state.push_assistant(
                                    "(prompt argument — not yet wired)".to_string(),
                                );
                                state.push_divider();
                            }
                        }
                    } else {
                        state.push_user(text);
                        state.push_divider();
                    }
                }
            }
        }
        KeyAction::Abort => {
            if state.completion.is_some() {
                // Cancel popup without changing input.
                state.completion = None;
            } else if state.mode == RunMode::Running {
                state.mode = RunMode::Aborted;
            } else {
                state.input.clear();
            }
        }
        KeyAction::Quit => state.mode = RunMode::Quitting,
        KeyAction::SwitchModel
        | KeyAction::ShowHelp
        | KeyAction::ScrollUp
        | KeyAction::ScrollDown => {}
        KeyAction::Noop => {}
    }
}

// Suppress unused warning for the dropped-join-handle field.
#[allow(dead_code)]
fn _typecheck_handle_is_used() {
    let _ = std::mem::size_of::<JoinHandle<()>>();
}
