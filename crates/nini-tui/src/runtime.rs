//! TUI runtime: alternate screen + raw mode + event loop + agent integration.
//!
//! Two main pieces:
//! - [`run_loop`]: drives the terminal in async mode, handles keystrokes
//!   and triggers redraws.
//! - [`AgentTask`]: spawned on Submit, runs the agent and pushes events
//!   into the shared state via [`AgentSink`].
#![allow(unused_mut)] // render/runtime use mut bindings for future hook points

use crate::keys::{resolve, Key, KeyAction};
use crate::render::render_frame_with_theme;
use crate::selector::SelectorItem;
use crate::state::{AppState, RunMode, TranscriptLine};
use anyhow::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{stdout, Stdout};
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

/// v0.9.1: acquire the shared state **without inheriting a poisoned
/// lock**.
///
/// The runtime had 28 `lock_state(&shared)` calls. A `Mutex` is
/// poisoned when a thread panics while holding it, and a poisoned mutex
/// makes *every* subsequent `lock()` return `Err`. So a single panic
/// anywhere in the state — a `no session` path, a formatting bug, an
/// arithmetic overflow (this release fixed three of those) — turned the
/// TUI into a panic loop: every subsequent frame, at the very first
/// `lock().unwrap()` on the render path, panicked again, and the user
/// was left staring at a frozen or blank terminal with no way out short
/// of killing the process.
///
/// A poisoned lock means "some state may be inconsistent", not "the
/// state is unreadable". The TUI re-derives almost everything it draws
/// from `AppState`'s own fields, and the failure mode of proceeding is a
/// transient glitch, while the failure mode of panicking is a dead
/// application. So: recover the guard and carry on.
///
/// This is why the failures above were survivable at all — but they were
/// survivable *until the next keypress*.
pub fn lock_state(shared: &SharedState) -> std::sync::MutexGuard<'_, AppState> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

/// Push a system notice into the transcript.
///
/// v0.10.3: the TUI used to write these to stderr, which is the
/// alternate screen while the TUI runs — the message landed on top of
/// the painted frame and read as garbage. A notice is a row in the
/// transcript instead: readable, scrollable, and unmistakably not model
/// output.
fn push_notice(shared: &SharedState, msg: impl Into<String>) {
    lock_state(shared).push_notice(msg);
}

/// A trimmed-down agent event for cross-task delivery. Avoids leaking
/// `nini_core::AgentEvent` (and its `AgentError`/`Provider` dependencies)
/// across the spawn boundary.
#[derive(Debug, Clone)]
pub enum AgentEventLite {
    TextDelta(String),
    /// v0.8: reasoning content from the model (inside `<think>`).
    /// Renders with dim/italic style in the transcript (Pi-style).
    ThinkingDelta(String),
    ToolCallStart {
        id: String,
        name: String,
    },
    ToolCallStop {
        id: String,
        args: String,
    },
    ToolResult {
        id: String,
        name: String,
        ok: bool,
        content: String,
        details: Option<serde_json::Value>,
        /// Wall-clock duration of the tool execution in milliseconds.
        /// Surfaced in the transcript as `Took N.Ns` (Pi-style) so
        /// the user can see how long each tool took.
        duration_ms: u64,
    },
    TurnEnd,
    /// v0.10 (R2h): the run returned an error, so the turn produced no
    /// assistant message at all.
    ///
    /// Distinct from `Error`, which is also used for transient notices
    /// ("retrying after transient error") that are not failures. A failed
    /// turn used to leave no trace in the session: `run()` returns `Err`,
    /// the driver pushes `Error`, and that path renders transcript-only, so
    /// a resumed session showed a conversation that had silently lost its
    /// last turn.
    TurnFailed(String),
    Error(String),
    /// v0.8.4 (ux-001): non-success stop reason surfaced at the end of
    /// an assistant turn. Mirrors Pi's `AssistantMessageComponent`
    /// end-of-turn notice ("Response was truncated before completion."
    /// / "Operation aborted" / "Error: …"). The string is a
    /// human-readable label that already maps to a friendly message.
    StopReason(String),
    /// v0.10 (R2e): tokens only. The cost used to travel as a third field,
    /// which `app.rs` filled with a hardcoded `0.0` - so `run_state.cost_usd`
    /// never moved and `render.rs`, which shows the cost pill only when
    /// `cost_usd > 0.0`, never showed one.
    ///
    /// Pricing moved here instead. The TUI is what knows the selected
    /// model, and the catalog is what knows its rates, so the two are in the
    /// same process and the arithmetic is testable through the sink. Sending
    /// a precomputed number across a boundary made it possible for that
    /// number to be a constant, which is exactly what happened.
    /// `input`, `output`, `cache_read`, `cache_write`.
    ///
    /// v0.10 (fix): the cache half was dropped on the floor. Pi prices and
    /// budgets a turn with `calculateContextTokens(usage) = totalTokens ||
    /// input + output + cacheRead + cacheWrite` (`compaction/compaction.ts:146`),
    /// and a cached read is most of the context on any provider that has
    /// prompt caching — so a two-field variant cannot compute the context
    /// bar, and the bar was therefore never drawn (`context_used` stayed at
    /// 0, so `render.rs` rendered `ctx 0%` or nothing at all).
    Usage(u32, u32, u32, u32),
    /// Agent phase transition. Mirrors `AgentEvent::PhaseChanged` from
    /// nini-core but as a lightweight payload (just the phase name).
    PhaseChanged(String),
    /// Marks end of agent run; flips state back to Editing.
    Done,
}

/// Cloneable handle for pushing agent events into the TUI state from a
/// background tokio task.
#[derive(Clone)]
pub struct AgentSink {
    state: SharedState,
    /// v0.8.5: shared `watch::Sender<bool>` so every agent task spawned
    /// during one runtime lifetime wakes the SAME runtime loop
    /// subscription. Replaces v0.7.4's per-iteration `Arc<Notify>`
    /// which was created inside `submit_user_input` and never wired
    /// to the runtime's select!, making the wake-on-event mechanism
    /// a silent no-op (notifications went to a Notify with zero
    /// listeners). The `watch` channel also deduplicates: 14 rapid
    /// pushes from parallel tool results collapse into a single
    /// `changed()` signal that the runtime consumes on its next
    /// iteration.
    notify: tokio::sync::watch::Sender<bool>,
}

impl AgentSink {
    pub fn new(state: SharedState, notify: tokio::sync::watch::Sender<bool>) -> Self {
        Self { state, notify }
    }

    /// The shared state behind this sink.
    ///
    /// v0.10 (R2a/A3b): the driver needs the session to compact through, and
    /// the sink is the only handle it is given per run. Pi's `AgentSession`
    /// holds both the agent and the session manager for exactly this reason.
    pub fn state(&self) -> SharedState {
        self.state.clone()
    }

    /// v0.10 (R1e): the model the user currently has selected.
    ///
    /// Read by `LiveConfigHooks::prepare_next_turn` on every completed
    /// turn. This is how a Ctrl+L / `/model` change made *while the agent
    /// is running* reaches the next provider request — before this, the
    /// status bar showed the new model while the loop kept calling the
    /// old one, because `RunConfig` was cloned once at `run()` entry.
    pub fn current_model(&self) -> Option<String> {
        let m = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .model_state
            .model
            .clone();
        // An empty selection would blank the model on the next request,
        // so treat it as "no opinion" and let the loop keep its model.
        if m.is_empty() {
            None
        } else {
            Some(m)
        }
    }

    /// Apply an `AgentEventLite` to the state.
    ///
    /// v0.8.5 (perf + correctness):
    ///   * **Single lock acquisition per event** (was: two). The
    ///     previous double-lock opened a race window where concurrent
    ///     pushes could interleave `PhaseChanged` and the rest of the
    ///     event, causing `status` to lag behind the actual transcript
    ///     update. Single lock also halves syscall cost during bursty
    ///     event streams (e.g. 14 parallel tool results).
    ///   * **Wakes the runtime via shared `watch` channel** (was:
    ///     per-iteration `Notify` that was wired to the wrong handle
    ///     and never fired).
    pub fn push(&self, ev: AgentEventLite) {
        let phase_status: Option<String> = if let AgentEventLite::PhaseChanged(phase) = &ev {
            Some(phase.clone())
        } else {
            None
        };
        if let Ok(mut s) = self.state.lock() {
            if let Some(phase) = phase_status {
                s.run_state.status = phase;
            }
            match ev {
                AgentEventLite::TextDelta(text) => {
                    s.push_assistant_raw(text.clone());
                    // P0.1: the transcript copy is display only; without
                    // this the model's prose never reached the session.
                    s.session_buffer_text(&text);
                }
                AgentEventLite::ThinkingDelta(text) => s.push_thinking_raw(text),
                AgentEventLite::ToolCallStart { id, name } => {
                    s.push_tool_call(id.clone(), name.clone(), "");
                    s.session_buffer_tool_call_start(&id, &name);
                }
                AgentEventLite::ToolCallStop { id, args } => {
                    // v0.8.8 (tool-identity): update the SPECIFIC tool
                    // call line by id, not by last position. Parallel
                    // tool calls (8× read in one turn) previously had
                    // every Stop event overwrite the LAST line, so
                    // only the final call's args survived — earlier
                    // calls' args were silently lost. Now we find the
                    // line by id and update in place.
                    let found = {
                        let mut found_idx: Option<usize> = None;
                        let lines = Arc::make_mut(&mut s.transcript_state);
                        for (i, line) in lines.lines.iter().enumerate().rev() {
                            if let TranscriptLine::ToolCall {
                                id: existing_id, ..
                            } = line
                            {
                                if existing_id == &id {
                                    found_idx = Some(i);
                                    break;
                                }
                            }
                        }
                        found_idx
                    };
                    if let Some(idx) = found {
                        let lines = Arc::make_mut(&mut s.transcript_state);
                        if let Some(TranscriptLine::ToolCall { args: a, .. }) =
                            lines.lines.get_mut(idx)
                        {
                            *a = args.clone();
                        }
                    } else {
                        // Stop without matching Start (out-of-order
                        // event from buggy provider). Fall back to
                        // pushing a new line; id is preserved for
                        // any later ToolResult to find.
                        s.push_tool_call(id.clone(), String::new(), args.clone());
                    }
                    // P0.1: the arguments arrive here, not at Start, so
                    // this is the first point where the call is complete.
                    s.session_buffer_tool_call_args(&id, &args);
                }
                AgentEventLite::ToolResult {
                    id,
                    name,
                    ok,
                    content,
                    details,
                    duration_ms,
                } => {
                    // Capture edit-tool diff stats for the status bar pill.
                    if let Some(d) = &details {
                        if let (Some(adds), Some(dels)) = (
                            d.get("additions").and_then(|v| v.as_u64()),
                            d.get("deletions").and_then(|v| v.as_u64()),
                        ) {
                            s.ui_state.last_diff = Some((adds as usize, dels as usize));
                        }
                    }
                    // v0.8.8 (tool-identity): ToolResult event carries the
                    // tool name so the TUI can render `✓ bash (4ms)`
                    // instead of just `✓`. This is critical when
                    // multiple tools run in parallel — the user needs
                    // to know which result belongs to which call.
                    s.push_tool_result_raw(
                        id.clone(),
                        name.clone(),
                        ok,
                        content.clone(),
                        Some(duration_ms),
                    );
                    // P0.1: the assistant message holding the call must be
                    // written before its result. This is the first moment the
                    // turn's shape is known.
                    s.session_flush_assistant();
                    s.session_append_tool_result(
                        &id,
                        &name,
                        ok,
                        &content,
                        details,
                        Some(duration_ms),
                    );
                }
                AgentEventLite::TurnEnd => {
                    s.push_divider();
                    // P0.1: a turn that made no tool call still has prose
                    // worth keeping, and nothing else would flush it.
                    s.session_flush_assistant();
                    // Flush session to disk on turn end. Errors are logged but
                    // never propagated — IO failures must not break the TUI.
                    s.session_flush();
                }
                AgentEventLite::Error(message) => {
                    s.push_assistant_raw(format!("[error] {message}"));
                }
                AgentEventLite::TurnFailed(message) => {
                    // Record it, so a resumed session can show that the turn
                    // failed rather than pretending it never happened.
                    s.session_append_turn_error(&message);
                    s.session_flush();
                    s.push_assistant_raw(format!("[error] {message}"));
                    s.push_divider();
                }
                AgentEventLite::StopReason(label) => {
                    Arc::make_mut(&mut s.transcript_state)
                        .lines
                        .push(TranscriptLine::StopNotice(label));
                }
                AgentEventLite::Usage(input, output, cache_read, cache_write) => {
                    s.run_state.tokens.input += input as u64;
                    s.run_state.tokens.output += output as u64;
                    // Priced against the model the TUI currently has
                    // selected, which is the one that produced this turn.
                    let cost = s.run_state.cost_usd
                        + nini_core::model_lookup::turn_cost(&s.model_state.model, input, output);
                    // v0.10 (fix): context used is the *latest* turn's
                    // context, not a running total — the next request sends
                    // the last context plus the answer to it, so a sum over
                    // turns would grow without bound.
                    //
                    // Routed through the setter that already existed for
                    // this and had no caller: the renderer reads the two
                    // fields, the setter is their only intended writer, and
                    // writing them here instead would have left the two
                    // paths able to disagree.
                    let window = s.run_state.context_window;
                    let used = input
                        .saturating_add(output)
                        .saturating_add(cache_read)
                        .saturating_add(cache_write);
                    s.set_status_bar_usage(cost, window, used);
                }
                AgentEventLite::PhaseChanged(_) => {
                    // Already handled at top of this match (set status).
                }
                AgentEventLite::Done => {
                    s.run_state.mode = RunMode::Editing;
                    s.run_state.status = "ready".to_string();
                    s.run_state.abort_signal = None; // clear stale abort signal
                }
            }
        } else {
            // v0.10.1 (diagnosis): the agent's events stopped reaching the
            // transcript after the first run in a long session, with no
            // error anywhere — and this `if let Ok` is the only place that
            // can swallow them. A poisoned mutex keeps returning `Err`
            // forever, so one panic anywhere in code that holds the state
            // lock silently disables every later turn. `lock_state` and
            // `LiveConfigHooks` recover poisoning via `into_inner`; this
            // path did not, which is why only the agent went quiet.
            tracing::error!(
                "agent event dropped: the state lock is poisoned — every later \
                 agent event is being discarded"
            );
        }
        // v0.8.5: notify via shared watch channel. send() ignores
        // the error if the receiver (runtime loop) has been dropped,
        // so we never block or panic here. Watch's "latest value"
        // semantics also coalesce 14+ rapid pushes from parallel tool
        // results into one `changed()` that the runtime drains on
        // its next iteration.
        let _ = self.notify.send(true);
    }

    /// Inject a tool result into the transcript synchronously. Used by local
    /// commands (!bash) that run outside the agent driver and need to contribute
    /// to the same turn's transcript without spawning an async task.
    pub fn inject_tool_result(&self, ok: bool, content: String) {
        if let Ok(mut s) = self.state.lock() {
            s.push_tool_result("placeholder", "placeholder", ok, content, None);
        }
    }
}

/// A function that drives one agent turn. Called by the runtime after Submit.
/// `sink` receives the agent's events. `done` is notified when the turn
/// finishes (whether successfully, with error, or aborted).
/// Drives one turn.
///
/// v0.10 (R2a/A3): takes the messages the session's live context resolves
/// to, so the Agent starts from the session rather than from an empty
/// history. This is Pi's `agent.state.messages = buildSessionContext(...)`,
/// moved to the boundary where the session is actually reachable — the
/// Agent has no session handle, and the driver used to hand it an empty
/// history while the session held the real one.
pub type AgentDriver = Arc<
    dyn Fn(String, Vec<nini_core::provider::Message>, AgentSink, Arc<Notify>) -> JoinHandle<()>
        + Send
        + Sync,
>;

/// Public entrypoint: run the TUI. `bootstrap` is called once to seed the
/// `AppState`. `agent_driver` is spawned whenever the user submits a message.
///
/// v0.8.8 (tool-identity): `initial_model` seeds `AppState::model` so the
/// status bar reflects the user-chosen model (e.g. `--model MiniMax-M3`).
/// Previously this was hardcoded to `"test-model"` and the user's `--model`
/// flag was silently ignored at the TUI layer (cf. v0.7.2 ux-report #7).
/// `bootstrap` may override the model again if the caller needs to.
/// Build the initial `AppState` the way a real session does.
///
/// v0.10 (R2c): extracted from `run` so the production sequence can be
/// tested rather than described. The sequence matters — the bootstrap is
/// what installs the settings snapshot, so the context window can only be
/// derived *after* it runs, and getting that order wrong leaves the window
/// at 0.
///
/// An earlier version of this test called `refresh_context_window` itself,
/// which meant it verified the method and not that anything called it: an
/// injection that deleted the call site from `run` passed clean. With the
/// sequence here, `run` and the test cannot diverge.
pub fn initial_state<F>(initial_model: &str, bootstrap: F) -> AppState
where
    F: FnOnce(&mut AppState),
{
    let mut state = AppState::new(initial_model);
    bootstrap(&mut state);
    state.refresh_context_window();
    state
}

/// The theme the session starts with, and the name the status bar shows.
///
/// v0.10: extracted from `run` for the same reason `initial_state` was
/// (see its doc comment) — the sequence lives inside an `async fn` behind a
/// TTY, so a test cannot reach it, which is exactly how
/// `SettingsManager::default()`'s hard-coded "dark" survived every test in
/// the file. Two callers must agree on the answer — startup and the theme
/// watcher — so the function both of them use is the thing under test.
pub fn startup_theme(shared: &SharedState) -> (String, crate::theme::Theme) {
    let name = {
        let g = lock_state(shared);
        let from_settings = g.model_state.settings_snapshot.theme.clone();
        if from_settings.trim().is_empty() {
            "dark".to_string()
        } else {
            from_settings
        }
    };
    let theme = crate::theme::Theme::resolve(&name);
    // Mirror the resolved name into AppState so the status bar shows what is
    // actually in use rather than what was asked for.
    lock_state(shared).ui_state.theme_name = Some(theme.name.clone());
    (theme.name.clone(), theme)
}

pub async fn run<F>(bootstrap: F, agent_driver: AgentDriver, initial_model: &str) -> Result<()>
where
    F: FnOnce(&mut AppState),
{
    let state = initial_state(initial_model, bootstrap);
    let shared = shared_state(state);

    // Install SIGINT/SIGTERM/SIGHUP handlers + panic hook before enabling
    // raw mode. The panic hook calls emergency_cleanup() to restore the
    // terminal if anything blows up.
    let _ = crate::signals::install_handlers();

    enable_raw_mode()?;
    crate::signals::mark_raw_mode(true);
    // v0.10.3: from here on, the library's stderr is a pipe, not the
    // terminal. The TUI owns the screen; anything else that writes to
    // it lands on top of the painted frame (the trust warning printed
    // on every tool call did exactly that, and the user saw two texts
    // fighting over one row). The guard turns those lines into
    // transcript notices.
    //
    // Installed after raw mode and before the alternate screen, and
    // dropped after the event loop returns — so a line that arrives
    // while the TUI is shutting down goes to the real terminal, not to
    // a screen that is about to be erased.
    let guard = crate::terminal_guard::TerminalGuard::install();
    let mut guard = match guard {
        Ok(g) => Some(g),
        Err(e) => {
            // A TUI without the guard is still a TUI — just a noisier
            // one. Say so where the reader will see it: a notice in the
            // transcript, not a line on the screen the TUI is about to
            // paint over.
            push_notice(&shared, format!("terminal guard not installed: {e}"));
            None
        }
    };
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    // v0.10: mouse capture, for wheel scrolling and dragging the transcript
    // scrollbar. crossterm's `EnableMouseCapture` emits
    // 1000+1002+1003+1015+1006; Pi uses 1000+1002+1004+1006 and
    // deliberately leaves out 1003 (all-motion reporting) so a stationary
    // pointer sends nothing. crossterm offers no narrower mode, so `Moved`
    // is dropped in `handle_mouse` instead. Recorded in the plan.
    execute!(stdout, EnableMouseCapture)?;
    crate::signals::mark_alt_screen(true);
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    terminal.hide_cursor()?;

    let result = run_loop(&mut terminal, shared, agent_driver, &mut guard).await;

    terminal.show_cursor()?;
    // Must go before leaving the alt screen or the shell inherits a
    // reporting mode and starts eating clicks.
    let _ = execute!(terminal.backend_mut(), DisableMouseCapture);
    // The transcript is about to disappear with the alternate screen, so
    // the guard must stop delivering to it **before** the screen goes
    // away — a line delivered after that point would be delivered to a
    // screen that no longer exists, which is the same as losing it.
    // The lines go to the real stderr instead.
    if let Some(g) = guard.as_ref() {
        g.begin_shutdown();
    }
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    disable_raw_mode()?;
    crate::signals::mark_raw_mode(false);
    crate::signals::mark_alt_screen(false);
    // The guard is dropped here, not before: the alternate screen is
    // already gone, so the lines that were still in the pipe land in
    // the user's scrollback instead of on a screen that no longer
    // exists.
    drop(guard);
    result
}

/// Main event loop. Returns when the user quits.
async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    shared: SharedState,
    agent_driver: AgentDriver,
    guard: &mut Option<crate::terminal_guard::TerminalGuard>,
) -> Result<()> {
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tick.tick().await; // skip the immediate first tick

    // F020: separate fast poll tick for the external editor flag.
    // 25ms feels snappy without flooding the lock with reads.
    let mut editor_poll_tick = tokio::time::interval(Duration::from_millis(25));
    editor_poll_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    editor_poll_tick.tick().await;

    // Selector state lives outside AppState so we can keep AppState Clone-able.
    let mut active_selector: Option<Box<dyn crate::selector::SelectorState + Send>> = None;
    let mut selector_query: String = String::new();
    let mut selector_visible: Vec<usize> = Vec::new();

    // Load the user's configured theme once. Hot-reload via theme watcher:
    // when a theme file changes, the watcher emits a `ThemeEvent` and we
    // reload from settings.
    let (_theme_name, mut theme) = startup_theme(&shared);

    // Spawn theme watcher (best-effort; falls back silently if dirs missing).
    let (theme_tx, mut theme_rx) = tokio::sync::mpsc::unbounded_channel();
    // Watch the file the settings were actually loaded from, not one
    // derived from `cwd`: they differ whenever settings come from the
    // agent directory, which is the usual case.
    let watched = lock_state(&shared).model_state.settings_path.clone();
    let _watcher = crate::theme_watcher::spawn_theme_watcher(watched, theme_tx);

    // v0.8.5: SINGLE shared `watch` channel between runtime and ALL
    // agent tasks spawned during this session. Replaces v0.7.4's
    // per-iteration `Arc<Notify>` which was created inside
    // `submit_user_input` and never wired to this loop's select! —
    // making the "wake-on-event" mechanism a silent no-op. Watch
    // coalesces: 14 rapid pushes from parallel tool results collapse
    // into one `changed()` that the loop drains on its next iteration.
    let (event_tx, mut event_rx) = tokio::sync::watch::channel(false);

    loop {
        // The guard's lines become transcript notices here, and only
        // here: the reader thread hands them over through its own
        // queue, so it never takes the state lock (a writer that
        // filled the pipe would then block) and a line is never lost
        // because the render loop held the lock at that instant.
        if let Some(g) = guard.as_ref() {
            for line in g.take_pending() {
                push_notice(&shared, line);
            }
        }
        // Snapshot state for rendering (cheap clone, doesn't hold lock long).
        // v0.8.5: large transcripts make this clone expensive; we still
        // hold the lock briefly. The watch channel above means we no
        // longer have to spin-lock on a Notify that nobody listens to.
        let snapshot = {
            let g = lock_state(&shared);
            g.clone()
        };
        // v0.6: F1 sets `state.ui_state.close_help` to ask the runtime to drop
        // any active help selector. Pick up that signal here so the
        // overlay actually disappears when the user toggles F1 again.
        if snapshot.ui_state.close_help {
            active_selector = None;
            selector_query.clear();
            selector_visible.clear();
            let mut g = lock_state(&shared);
            g.ui_state.close_help = false;
            drop(g);
        }
        // Compute selector visible indices (for rendering).
        if let Some(sel) = active_selector.as_ref() {
            let items = sel.state_items();
            if selector_query.is_empty() {
                selector_visible = (0..items.len()).collect();
            } else {
                selector_visible = crate::selector::fuzzy_filter(&selector_query, &items);
            }
        }
        let selector_title = active_selector
            .as_ref()
            .map(|s| s.state_title().to_string());
        let selector_items: Vec<SelectorItem> = active_selector
            .as_ref()
            .map(|s| s.state_items())
            .unwrap_or_default();
        let selector_selected = active_selector
            .as_ref()
            .map(|s| s.state_selected())
            .unwrap_or(0);

        terminal.draw(|f| {
            render_frame_with_theme(f, &snapshot, &theme);
            // Overlay the selector panel if active. Crucially, the
            // selector only covers the *transcript* area — never the
            // input bar or footer. v0.5 used `height = area.height - 4`
            // which left the selector's bottom edge overlapping the
            // input row, producing the `i│put` / transcript-bleed
            // glitch reported in the v0.6 UX survey.
            if let Some(title) = selector_title.clone() {
                let area = f.area();
                // Same vertical layout as render_frame_with_theme:
                //   [status 1] [transcript N] [prompt 3] [footer 1]
                // Selector fills the transcript area only.
                let selector_area = if area.height >= 5 {
                    ratatui::layout::Rect {
                        x: area.x + 2,
                        y: area.y + 1,
                        width: area.width.saturating_sub(4),
                        // Subtract status(1) + prompt(3) + footer(1) +
                        // 1 row padding so the selector doesn't bleed.
                        height: area.height.saturating_sub(6),
                    }
                } else {
                    area
                };
                crate::render::render_selector_panel(
                    f,
                    &title,
                    &selector_query,
                    &selector_items,
                    &selector_visible,
                    selector_selected,
                    &theme,
                    selector_area,
                );
            }
        })?;

        let done = Arc::new(Notify::new()); // per-iteration done signal
        let done_for_select = done.clone();
        // v0.8.5: sink_notify removed (was per-iteration, never wired).
        // The shared `event_rx` above is the runtime's wake-up channel
        // for all agent events, regardless of which iteration spawned
        // them.

        // Check if submit_user_input signaled a selector-open request via
        // state.run_state.status. Run AFTER done.notify_waiters() in submit_user_input.
        if active_selector.is_none() {
            let status = lock_state(&shared).run_state.status.clone();
            if let Some(kind) = status.strip_prefix("open_selector:") {
                let current_model = lock_state(&shared).model_state.model.clone();
                match kind {
                    "model" => {
                        let sel =
                            Box::new(crate::selectors::ModelSelector::new(Some(&current_model)));
                        active_selector = Some(sel);
                        selector_query.clear();
                    }
                    "thinking" => {
                        // Read current thinking level from settings.
                        let cur = {
                            let mut s = crate::settings::SettingsManager::default();
                            s.theme_name(); // force load
                            s.get().default_thinking_level.clone().unwrap_or_default()
                        };
                        let sel = Box::new(crate::selectors::ThinkingSelector::new(Some(&cur)));
                        active_selector = Some(sel);
                        selector_query.clear();
                    }
                    "session" => {
                        let cwd = std::env::current_dir().ok();
                        let cwd_name = cwd
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "default".to_string());
                        // An empty directory lists nothing, rather than
                        // scanning a path built from `$HOME` directly -
                        // which is what this did, so `NINI_AGENT_DIR` had
                        // no effect on the session picker at all.
                        let dir = nini_core::paths::sessions_dir()
                            .map(|b| b.join(&cwd_name))
                            .unwrap_or_default();
                        let sel = Box::new(crate::selectors::SessionSelector::from_dir(&dir));
                        active_selector = Some(sel);
                        selector_query.clear();
                    }
                    "tree" => {
                        // Open tree selector against the active session's entries.
                        let session_arc = lock_state(&shared).session_state.session.clone();
                        let entries: Vec<nini_core::SessionEntry> = if let Some(arc) = session_arc {
                            // v0.10 (fix): was `try_lock` with an empty
                            // `else`, so a `/tree` opened while a turn was
                            // being appended showed an empty tree — which
                            // is indistinguishable from "this session has
                            // no branches".
                            crate::state::lock_session(&arc).entries.clone()
                        } else {
                            Vec::new()
                        };
                        let sel = Box::new(crate::selectors::TreeSelector::from_entries(&entries));
                        active_selector = Some(sel);
                        selector_query.clear();
                    }
                    "trust" => {
                        let cwd = std::env::current_dir().ok();
                        let cwd_str = cwd
                            .as_ref()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let trust_path = nini_core::paths::trust_file().and_then(|p| {
                            nini_core::project_trust::ProjectTrustStore::load(&p).ok()
                        });
                        let current = trust_path.as_ref().and_then(|t| t.get(&cwd_str));
                        let sel = Box::new(crate::selectors::TrustSelector::new(cwd_str, current));
                        active_selector = Some(sel);
                        selector_query.clear();
                    }
                    "settings" => {
                        // Built from `SettingsManager::default()`, so it
                        // listed a fabricated model, theme and thinking
                        // level no matter what the user had configured -
                        // the same throwaway-default shape as the
                        // `/thinking` selector had.
                        let path = lock_state(&shared).model_state.settings_path.clone();
                        let manager = match path {
                            Some(p) => crate::settings::SettingsManager::load_from_disk(p),
                            None => crate::settings::SettingsManager::default(),
                        };
                        let sel = Box::new(crate::selectors::SettingsSelector::new(manager));
                        active_selector = Some(sel);
                        selector_query.clear();
                    }
                    "help" => {
                        // v0.6: real /help overlay (was a 1-line status
                        // string in v0.5). Lists every registered slash
                        // command with fuzzy filter + description.
                        let sel = Box::new(crate::help_overlay::HelpSelector::new());
                        active_selector = Some(sel);
                        selector_query.clear();
                    }
                    "palette" => {
                        // F015 Ctrl+K command palette: fuzzy-search
                        // every command + meta action. Enter on a
                        // hit dispatches it (handled in
                        // apply_selector_result).
                        let sel = Box::new(crate::command_palette::CommandPalette::new());
                        active_selector = Some(sel);
                        selector_query.clear();
                    }
                    _ => {}
                }
                // Clear the status flag so it doesn't re-trigger.
                lock_state(&shared).run_state.status = "ready".to_string();
            }
        }

        tokio::select! {
            maybe = events.next() => {
                match maybe {
                    Some(Ok(Event::Key(k))) => {
                        // If a selector is active, route keys to it instead.
                        if active_selector.is_some() {
                            handle_selector_key(
                                k,
                                &shared,
                                active_selector.as_mut().unwrap(),
                                &mut selector_query,
                                &mut selector_visible,
                            );
                            // Check if selector closed itself.
                            if selector_visible.is_empty() && selector_query.is_empty() {
                                // Selector was confirmed or cancelled — apply result.
                                let model_update = if let Some(s) = active_selector.take() {
                                    apply_selector_result(s, &shared)
                                } else {
                                    None
                                };
                                if let Some(new_model) = model_update {
                                    lock_state(&shared).set_model(new_model);
                                }
                                // Selector closed: reset any residual
                                // open_selector:* flag so the status bar
                                // doesn't display stale 'switch model…'
                                // / 'open_selector:tree' after the user
                                // has navigated away.
                                clear_status_after_selector(&shared);
                                if let Ok(mut g) = shared.lock() {
                                    if g.run_state.status.is_empty() || g.run_state.status.starts_with("open_selector:") {
                                        g.run_state.status = "ready".to_string();
                                    }
                                }
                            }
                        } else {
                            handle_key(k, &shared, &agent_driver, done, event_tx.clone());
                        }
                    }
                    Some(Ok(Event::Mouse(m))) => {
                        let _ = handle_mouse(m, &shared);
                    }
                    Some(Ok(Event::Resize(_, _))) => { /* ratatui handles */ }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => push_notice(&shared, format!("event error: {e}")),
                    None => break,
                }
            }
            _ = done_for_select.notified() => {
                // Agent finished; loop will redraw on next iteration.
            }
            _ = event_rx.changed() => {
                // v0.8.5: agent emitted one or more events since last
                // redraw. Watch coalesces — one `changed()` covers
                // bursts (e.g. 14 parallel tool results). Mark seen
                // so the next iteration can detect new changes.
                let _ = event_rx.borrow_and_update();
            }
            // F020: external editor dance. Polled each iteration
            // because the dance is synchronous (we leave alt screen
            // + spawn editor + re-enter) so a Notify wouldn't fire
            // until after we'd already resumed anyway. We just
            // check the flag cheaply here.
            _ = editor_poll_tick.tick() => {
                if lock_state(&shared).run_state.pending_external_editor {
                    handle_external_editor_dance(terminal, &shared, guard);
                }
            }
            _ = tick.tick() => { /* animation tick */ }
            Some(theme_event) = theme_rx.recv() => {
                // Theme file changed — reload from settings.
                use crate::theme_watcher::ThemeEvent;
                match theme_event {
                    ThemeEvent::Changed(_) | ThemeEvent::Removed(_) => {
                        // `SettingsManager::default()`: the reload read a
                        // *default* manager, not the user's settings, so
                        // this would have replaced whatever theme they had
                        // with "dark". Same throwaway-default shape as
                        // `/thinking` and `/settings` had.
                        let path = lock_state(&shared).model_state.settings_path.clone();
                        let mut settings = match path {
                            Some(p) => crate::settings::SettingsManager::load_from_disk(p),
                            None => crate::settings::SettingsManager::default(),
                        };
                        let name = settings.theme_name();
                        lock_state(&shared).ui_state.theme_name = name.clone();
                        theme = settings.theme();
                        // `name` is applied above; all that was left
                        // here was formatting a status line the event
                        // bus used to receive and discard.
                        let _ = name;
                    }
                }
            }
        }

        let mode = lock_state(&shared).run_state.mode;
        if mode == RunMode::Quitting {
            return Ok(());
        }
    }
    Ok(())
}

/// Map a `crossterm::KeyEvent` to a `KeyAction` and apply to state.
/// On Submit (in Editing mode), spawn the agent runner.
/// Handle a key press while a selector is active. Updates `query`,
/// `visible`, and the selector's selected index. When user confirms (Enter)
/// or cancels (Esc), clears the query so the caller can detect closure.
fn handle_selector_key(
    k: KeyEvent,
    _shared: &SharedState,
    selector: &mut Box<dyn crate::selector::SelectorState + Send>,
    query: &mut String,
    visible: &mut Vec<usize>,
) {
    use crate::keys::Key as K;
    let key: K = k.into();
    match key.code {
        crossterm::event::KeyCode::Up => {
            // visible holds *indices into the full items list*, so we
            // navigate by position within visible, then translate back
            // to the item index. v0.5's bug: it treated cur as both a
            // visible position AND an items index, so after typing a
            // filter the highlighted item jumped around unexpectedly.
            if visible.is_empty() {
                return;
            }
            let cur = selector.state_selected();
            let cur_pos = visible.iter().position(|&i| i == cur).unwrap_or(0);
            let new_pos = if cur_pos == 0 {
                visible.len() - 1
            } else {
                cur_pos - 1
            };
            selector.state_set_selected(visible[new_pos]);
        }
        crossterm::event::KeyCode::Down => {
            if visible.is_empty() {
                return;
            }
            let cur = selector.state_selected();
            let cur_pos = visible.iter().position(|&i| i == cur).unwrap_or(0);
            let new_pos = (cur_pos + 1) % visible.len();
            selector.state_set_selected(visible[new_pos]);
        }
        crossterm::event::KeyCode::Backspace => {
            query.pop();
            *visible = compute_visible(query, &selector.state_items());
            // After filter change, keep selection valid (clamp to first
            // visible item) so Enter picks what the user actually sees.
            if let Some(&first) = visible.first() {
                selector.state_set_selected(first);
            }
        }
        crossterm::event::KeyCode::Esc => {
            query.clear();
            visible.clear();
        }
        crossterm::event::KeyCode::Enter => {
            // Translate the items-level `selected` index through the
            // current `visible` filter so the runtime picks what the
            // user sees highlighted.
            let outcome = if !visible.is_empty() {
                let cur = selector.state_selected();
                if !visible.contains(&cur) {
                    selector.state_set_selected(visible[0]);
                }
                selector.state_on_select()
            } else {
                selector.state_on_select()
            };
            match outcome {
                crate::selector::SelectorOutcome::Picked(_)
                | crate::selector::SelectorOutcome::Back => {
                    query.clear();
                    visible.clear();
                }
                crate::selector::SelectorOutcome::Cancelled => {
                    query.clear();
                    visible.clear();
                }
            }
        }
        crossterm::event::KeyCode::Char(c) => {
            query.push(c);
            *visible = compute_visible(query, &selector.state_items());
            // Reset selection to first visible item so Enter picks the
            // top match (matches the prior "always pick what I see" UX).
            if let Some(&first) = visible.first() {
                selector.state_set_selected(first);
            }
        }
        _ => {}
    }
}

fn compute_visible(query: &str, items: &[crate::selector::SelectorItem]) -> Vec<usize> {
    if query.is_empty() {
        return (0..items.len()).collect();
    }
    // Cheap substring match first; fall back to fuzzy.
    let q = query.to_lowercase();
    let mut exact: Vec<usize> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if item.label.to_lowercase().contains(&q) {
            exact.push(i);
        }
    }
    if !exact.is_empty() {
        return exact;
    }
    crate::selector::fuzzy_filter(query, items)
}

/// Helper for early-return in `submit_user_input`: drop state lock and
/// notify `done` (so the runtime loop wakes up). The actual selector open
/// happens in the loop body (which can mutate its `active_selector`
/// without deadlock).
fn drop_and_signal(_text: String, done: Arc<Notify>) {
    done.notify_waiters();
}

/// Apply a confirmed selector's result to AppState. Returns the new model
/// name if the selector was ModelSelector (caller writes to state).
/// Persist a thinking level for the next run.
///
/// Loads the existing file first: `flush` writes the whole `Settings`, so
/// starting from a default manager would erase the user's theme and model
/// to record one word about thinking.
///
/// Without a settings path there is nowhere to write, and this does not
/// invent one - it reports, and the caller says so.
/// Persist a thinking level for the next run.
///
/// Loads the existing file first: `flush` writes the whole `Settings`, so
/// starting from a default manager would erase the user's theme and model
/// to record one word about thinking.
///
/// Without a settings path there is nowhere to write, and this does not
/// invent one - it reports, and the caller says so.
///
/// v0.10.3: the report used to be an `eprintln!`, which lands on the
/// alternate screen the TUI is painting. It is returned instead, and the
/// caller puts it in the transcript as a notice.
fn persist_thinking_level(path: Option<&std::path::Path>, level: &str) -> Option<String> {
    let Some(path) = path else {
        return Some(format!(
            "thinking level set to {level} for this session only (no settings file)"
        ));
    };
    let mut settings = crate::settings::SettingsManager::load_from_disk(path.to_path_buf());
    settings.set_default_thinking_level(level);
    if let Err(e) = settings.flush() {
        return Some(format!("thinking level not saved: {e}"));
    }
    None
}

fn apply_selector_result(
    mut selector: Box<dyn crate::selector::SelectorState + Send>,
    shared: &SharedState,
) -> Option<String> {
    // Downcast to concrete selectors to read their `result` field, then
    // persist via SettingsManager / ProjectTrustStore / etc.
    use crate::selectors::model::ModelSelector;
    use crate::selectors::session::SessionSelector;
    use crate::selectors::settings::SettingsSelector;
    use crate::selectors::thinking::ThinkingSelector;
    use crate::selectors::tree::TreeSelector;
    use crate::selectors::trust::TrustSelector;
    use crate::settings::SettingsManager;

    if let Some(model_sel) = selector.state_as_any_mut().downcast_mut::<ModelSelector>() {
        let model = model_sel.result.clone();
        if let Some(m) = &model {
            let mut settings = SettingsManager::default();
            settings.set_default_model(m.clone());
        }
        return model;
    }
    if let Some(think_sel) = selector
        .state_as_any_mut()
        .downcast_mut::<ThinkingSelector>()
    {
        if let Some(level) = &think_sel.result {
            // This built `SettingsManager::default()`, wrote the level into
            // it, and dropped it. Nothing was loaded from the settings file
            // and nothing was written back, so choosing a level from the
            // selector had no effect at all.
            let path = lock_state(shared).model_state.settings_path.clone();
            if let Some(msg) = persist_thinking_level(path.as_deref(), level) {
                push_notice(shared, msg);
            }
            lock_state(shared).model_state.thinking_level = Some(level.clone());
        }
        return None;
    }
    if let Some(trust_sel) = selector.state_as_any_mut().downcast_mut::<TrustSelector>() {
        if let Some(decision) = trust_sel.result {
            // Persist to ProjectTrustStore.
            let cwd = trust_sel.cwd.clone();
            if let Some(path) = nini_core::paths::trust_file() {
                let mut store =
                    nini_core::project_trust::ProjectTrustStore::load(&path).unwrap_or_default();
                store.set(&cwd, decision);
                if let Err(e) = store.save(&path) {
                    // This was `let _ =` on a `save` that wrote nothing at
                    // all, so a trust decision that could not be recorded
                    // looked exactly like one that had been.
                    push_notice(
                        shared,
                        format!("trust: could not write {}: {e}", path.display()),
                    );
                }
            }
        }
        return None;
    }
    if let Some(session_sel) = selector
        .state_as_any_mut()
        .downcast_mut::<SessionSelector>()
    {
        if let Some(path_str) = session_sel.result.clone() {
            // Load the session file into AppState.
            let path = std::path::PathBuf::from(&path_str);
            // The lock is taken and released before the notice is
            // pushed: `Mutex` is not reentrant, and the notice needs
            // the very lock `session_load` was holding.
            let loaded = lock_state(&shared).session_load(path);
            if let Err(e) = loaded {
                push_notice(shared, format!("session load failed: {e}"));
            }
        }
        return None;
    }
    if let Some(tree_sel) = selector.state_as_any_mut().downcast_mut::<TreeSelector>() {
        // Compute branch summary for the picked branch and:
        // (a) surface a preview in the transcript,
        // (b) queue the full summary into pending_next_turn_messages so
        //     the next user turn has it as context (Pi parity — mirrors
        //     _pendingNextTurnMessages).
        let session_arc = lock_state(&shared).session_state.session.clone();
        let prev_status = lock_state(&shared).run_state.status.clone();
        lock_state(&shared).run_state.status = "branch summary: computing…".into();
        if let Some(arc) = session_arc {
            // v0.10 (fix): was `if let Ok(guard) = arc.try_lock()`, and the
            // summary is the point of picking a branch — a skipped one left
            // the status line reading "branch summary: computing…" forever,
            // because the status was restored inside the same `if let`.
            let entries = crate::state::lock_session(&arc).entries.clone();
            {
                let summary = tree_sel.summarize_at(0, &entries);
                if let Some(s) = summary {
                    // (a) Preview in transcript.
                    let truncated: String = if s.len() > 400 {
                        let mut t = s.clone();
                        t.truncate(400);
                        t.push_str("…");
                        t
                    } else {
                        s.clone()
                    };
                    let len = truncated.len();
                    let mut g = lock_state(&shared);
                    g.push_assistant(format!("(branch summary: {len} chars)\n{truncated}",));
                    g.push_divider();
                    // (b) Queue full summary for next turn.
                    g.run_state
                        .pending_next_turn_messages
                        .push(format!("[BRANCH SUMMARY]\n\n{}", s,));
                }
                lock_state(&shared).run_state.status = prev_status;
            }
        }
        return None;
    }
    if let Some(settings_sel) = selector
        .state_as_any_mut()
        .downcast_mut::<SettingsSelector>()
    {
        // The comment here used to claim "the settings manager inside the
        // selector persists to disk via its internal mechanism". There is
        // no manager inside the selector, only a snapshot, and nothing was
        // persisted. It then called `apply(0)`, which ignored its index, so
        // picking "theme" or "thinking" returned the model name and this
        // function reported a model change that had not happened.
        //
        // What it returns now is the category the user picked. No
        // sub-selector is opened yet - see the module doc in
        // `selectors/settings.rs` - so nothing else changes.
        let _picked = settings_sel
            .picked_category(settings_sel.selected)
            .map(str::to_string);
        None
    } else if let Some(palette) = selector
        .state_as_any_mut()
        .downcast_mut::<crate::command_palette::CommandPalette>()
    {
        // F015: the user picked a palette entry. Read the selected
        // item and dispatch based on its id prefix.
        use crate::selector::SelectorState;
        let selected_idx = palette.state_selected();
        let items = palette.state_items();
        if let Some(item) = items.get(selected_idx) {
            // Read the id first so we can drop the lock before
            // mutating shared state via dispatch().
            let id = item.value.clone();
            let label = item.label.clone();
            match id.as_str() {
                id if id.starts_with("cmd:/") => {
                    // Inject the slash command into the input buffer
                    // and call submit_user_input via the same path
                    // the runtime uses for an Enter keypress.
                    let name = id.trim_start_matches("cmd:/").to_string();
                    let cmd_line = format!("/{name}");
                    let mut g = lock_state(&shared);
                    g.input.text.clear();
                    g.input.cursor = 0;
                    drop(g);
                    if let Some((cmd_id, args)) = crate::commands::parse(&cmd_line) {
                        let mut g2 = lock_state(&shared);
                        let mut settings = crate::settings::SettingsManager::default();
                        let result =
                            crate::commands::dispatch(&mut g2, &mut settings, cmd_id, &args);
                        // Mirror the runtime's submit_user_input Output
                        // branch so palette-dispatched commands actually
                        // show their output.
                        if let Some(err) = result.error {
                            g2.push_assistant(format!("[command error] {err}"));
                            g2.push_divider();
                        }
                        match result.outcome {
                            crate::commands::CommandOutcome::Output(lines) => {
                                for line in lines {
                                    g2.push_assistant(line);
                                }
                                g2.push_divider();
                            }
                            crate::commands::CommandOutcome::Quit => {
                                g2.run_state.mode = crate::state::RunMode::Quitting;
                            }
                            crate::commands::CommandOutcome::PromptArgument { prompt, next: _ } => {
                                g2.push_assistant(format!("(prompt: {prompt})"));
                                g2.push_divider();
                            }
                        }
                    }
                }
                "action:clear" => {
                    let mut g = lock_state(&shared);
                    // v0.8.9 (P1-5): a stale `scroll_offset` from a
                    // longer transcript would survive the clear and
                    // render an empty box.
                    let ts = Arc::make_mut(&mut g.transcript_state);
                    ts.lines.clear();
                    ts.reset_view();
                    // v0.8.9 (perf): the line indices are about to be
                    // reused by new content.
                    g.invalidate_item_counts();
                    g.push_divider();
                }
                "action:exit" => {
                    let mut g = lock_state(&shared);
                    g.run_state.mode = RunMode::Quitting;
                }
                _ => {
                    let mut g = lock_state(&shared);
                    g.push_assistant(format!("[palette] unknown action id: {}", id));
                    g.push_divider();
                    let _ = label;
                }
            }
        }
        None
    } else {
        None
    }
}

// Status-string cleanup: every selector branch above eventually falls
// through to here; we reset state.run_state.status to "ready" so that stale
// strings like "switch model (not yet implemented)" or
// "open_selector:model" don't linger after the selector closes.
fn clear_status_after_selector(shared: &SharedState) {
    if let Ok(mut g) = shared.lock() {
        if g.run_state.status.starts_with("open_selector:")
            || g.run_state.status == "switch model (not yet implemented)"
        {
            g.run_state.status = "ready".to_string();
        }
    }
}

/// F020: handle Ctrl+G / `/editor` — the external editor dance.
///
/// We:
///   1. Snapshot the current input buffer.
///   2. Leave the alternate screen + disable raw mode + show cursor.
///   3. Spawn `$VISUAL` / `$EDITOR` / `nano` / `vi` on a temp file
///      pre-populated with the snapshot.
///   4. Read the file back; if it changed, replace the input buffer.
///   5. Re-enter the alternate screen + enable raw mode + hide cursor
///      + force a full redraw (the alternate screen is wiped on entry).
///
/// Anything that can fail does so gracefully — we surface the error
/// to the status bar / transcript and continue. The TUI itself is
/// restored unconditionally via a try-finally-style pattern so a
/// crash in the editor child doesn't leave the user with a broken
/// terminal.
fn handle_external_editor_dance(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    shared: &SharedState,
    guard: &mut Option<crate::terminal_guard::TerminalGuard>,
) {
    // 1. Snapshot input and clear the flag.
    let (initial_text, initial_cursor) = {
        let mut g = lock_state(&shared);
        g.run_state.pending_external_editor = false;
        (g.input.text.clone(), g.input.cursor)
    };

    // 2. Suspend the TUI.
    // We deliberately use execute! on stdout rather than on the
    // terminal's backend: crossterm's LeaveAlternateScreen /
    // disable_raw_mode write to the file descriptor directly,
    // and the terminal wrapper holds its own buffered copy. Going
    // through execute!() bypasses the buffer and reaches the real
    // stdout immediately.
    {
        let mut stdout = stdout();
        let _ = execute!(stdout, DisableMouseCapture);
        let _ = execute!(stdout, LeaveAlternateScreen);
        let _ = disable_raw_mode();
        crate::signals::mark_alt_screen(false);
        crate::signals::mark_raw_mode(false);
        let _ = execute!(stdout, crossterm::cursor::Show);
    }

    // 3. The editor runs in the user's **real** terminal, not in the
    // TUI's. The guard would swallow the editor's diagnostics — `vim:
    // error reading ...`, `code: command not found` — into a
    // transcript that is *not on screen* right now, so the user would
    // never see them. The guard is therefore taken down for the
    // duration of the editor and put back afterwards.
    crate::terminal_guard::suspend_for_external_editor(guard);

    // 4. Spawn the editor synchronously. This call blocks the
    // run loop until the user finishes editing. That's OK —
    // the TUI is suspended, so no UI updates are expected.
    let result = crate::editor::edit_in_external_editor(&initial_text);

    // 5. Put the guard back before the TUI paints again: the library's
    // stderr must not land on the frame we are about to draw. If the
    // guard cannot be installed, the TUI keeps running without it —
    // noisier, but not broken — and says so in the transcript.
    if let Err(e) = crate::terminal_guard::resume_after_external_editor(guard) {
        push_notice(
            shared,
            format!("terminal guard not reinstalled after the editor: {e}"),
        );
    }

    // 6. Resume the TUI BEFORE applying the result. We must be
    // able to write to the terminal again to redraw the new state.
    {
        let mut stdout = stdout();
        let _ = enable_raw_mode();
        crate::signals::mark_raw_mode(true);
        let _ = execute!(stdout, EnterAlternateScreen);
        crate::signals::mark_alt_screen(true);
        let _ = execute!(stdout, crossterm::cursor::Hide);
    }

    // 7. Apply the result (or the error) and force a redraw.
    match result {
        Ok(Some(new_text)) => {
            let mut g = lock_state(&shared);
            // Replace the entire input buffer + push an undo snapshot
            // so Ctrl+Z restores the pre-edit version.
            g.input.replace_whole(new_text.clone());
            g.run_state.status = format!("editor: {} chars", new_text.chars().count());
        }
        Ok(None) => {
            // No change.
            let mut g = lock_state(&shared);
            g.run_state.status = "editor: no changes".to_string();
            // Suppress the unused warning on initial_cursor.
            let _ = initial_cursor;
        }
        Err(e) => {
            let mut g = lock_state(&shared);
            g.run_state.status = format!("editor error: {e}");
            g.push_assistant(format!("[editor error] {e}"));
            g.push_divider();
        }
    }

    // 8. Force the next render — terminal.clear() wipes any stale
    // content from the editor session.
    let _ = terminal.clear();
}

/// Cycle to the next/previous model in `state.model_state.models_cycle`.
/// Updates `state.model_state.model`, persists to settings.json, and updates status.
fn cycle_model(state: &mut crate::state::AppState, direction: i32) {
    if state.model_state.models_cycle.is_empty() {
        state.run_state.status = "(no model cycle configured; use /model)".to_string();
        return;
    }
    // Find current model in cycle; advance by `direction`.
    let current_pos = state
        .model_state
        .models_cycle
        .iter()
        .position(|m| m == &state.model_state.model)
        .unwrap_or(0);
    let n = state.model_state.models_cycle.len() as i32;
    let mut new_pos = current_pos as i32 + direction;
    if new_pos < 0 {
        new_pos += n;
    } else if new_pos >= n {
        new_pos -= n;
    }
    let new_pos = new_pos as usize;
    state.model_state.models_cycle_idx = Some(new_pos);
    let new_model = state.model_state.models_cycle[new_pos].clone();
    state.set_model(new_model.clone());
    // Persist to settings.json if the runtime wired in a real path.
    let mut settings = match state.model_state.settings_path.clone() {
        Some(p) => crate::settings::SettingsManager::load_from_disk(p),
        None => crate::settings::SettingsManager::default(),
    };
    settings.set_default_model(&new_model);
    state.run_state.status = format!("model: {new_model}");
}

/// Cycle thinking level through the standard set.
fn cycle_thinking(state: &mut crate::state::AppState, direction: i32) {
    const LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];
    // Read current level from dedicated state field; fall back to "medium"
    // if never set.
    let current = state
        .model_state
        .thinking_level
        .as_deref()
        .unwrap_or("medium");
    let current_pos = LEVELS.iter().position(|l| *l == current).unwrap_or(3);
    let n = LEVELS.len() as i32;
    let mut new_pos = current_pos as i32 + direction;
    if new_pos < 0 {
        new_pos += n;
    } else if new_pos >= n {
        new_pos -= n;
    }
    let new_level = LEVELS[new_pos as usize];
    // The per-model branch called `set_model_thinking_level`, an empty
    // function, so `/thinking` persisted nothing whenever a model was
    // selected - the normal case. And neither branch called `flush`, so
    // even the working one never reached disk.
    //
    // There is no per-model level to write. Pi persists one knob,
    // `defaultThinkingLevel`
    // (`pi-coding-agent/src/core/settings-manager.ts:753`); its per-model
    // levels come from the `--models` CLI scope (`core/model-resolver.ts:
    // 295`, `provider/*:high`), a runtime concept nini does not have.
    if let Some(msg) = persist_thinking_level(state.model_state.settings_path.as_deref(), new_level)
    {
        state.push_notice(msg);
    }
    // Also update live state so the selector stays in sync.
    state.model_state.thinking_level = Some(new_level.to_string());
    state.run_state.status = format!("thinking: {new_level}");
}

fn handle_key(
    k: KeyEvent,
    shared: &SharedState,
    agent_driver: &AgentDriver,
    done: Arc<Notify>,
    event_tx: tokio::sync::watch::Sender<bool>,
) {
    let mut state = lock_state(&shared);
    let key: Key = k.into();
    let action = resolve_with_user_overrides(key);
    // Drop the lock while we hold it; the rest of the match needs it.

    match action {
        KeyAction::Insert(c) => {
            // F019: search-mode special keys (n/N) jump matches.
            if state.ui_state.search.is_some() {
                if c == 'n' {
                    state.search_next();
                    return;
                } else if c == 'N' {
                    state.search_prev();
                    return;
                }
                // Otherwise treat as query char.
                if let Some(search) = state.ui_state.search.as_mut() {
                    search.query.push(c);
                }
                let q = state
                    .ui_state
                    .search
                    .as_ref()
                    .map(|s| s.query.clone())
                    .unwrap_or_default();
                state.update_search_query(q);
                return;
            }
            // v0.10.1 (M1): no mode gate. The neighbours — Backspace,
            // Delete, Move*, Kill*, Undo, ClearInput — never had one, so a
            // running turn could delete text and move the caret but could
            // not type a character. Pi's editor is a separate component and
            // `isStreaming` never decides whether it accepts keystrokes
            // (`interactive-mode.js:2491`).
            state.input.insert_char(c);
            state.refresh_completion();
        }
        KeyAction::OpenSearch => {
            // F019: open search when input is empty AND no popup is up;
            // otherwise insert literal `/` so the user can still type
            // a slash command.
            if state.run_state.mode == RunMode::Editing
                && state.ui_state.completion.is_none()
                && state.input.text.is_empty()
            {
                state.begin_search();
            } else if state.run_state.mode == RunMode::Editing {
                state.input.insert_char('/');
                state.refresh_completion();
            }
        }
        KeyAction::OpenCommandPalette => {
            // F015 Ctrl+K: open the command palette. Works from any
            // editing state; closes any open slash popup first.
            if state.run_state.mode == RunMode::Editing {
                state.ui_state.completion = None;
                state.run_state.status = "open_selector:palette".to_string();
            }
        }
        KeyAction::PasteImage => {
            // Try to read an image from the system clipboard. If found,
            // insert its `[pasted image: <path>]` description into the
            // prompt AND surface a short transcript confirmation so
            // the user sees what landed. If the clipboard has no image
            // (or is unavailable in headless env), silently fall
            // through to Noop.
            match crate::image_paste::paste_image_with_size_from_clipboard() {
                Ok(Some((path, size))) => {
                    let desc = crate::image_paste::describe_path(&path);
                    // Insert the description char-by-char into the
                    // prompt input.
                    for c in desc.chars() {
                        state.input.insert_char(c);
                    }
                    state.refresh_completion();
                    // Surface a short echo in the transcript.
                    state.push_assistant(format!(
                        "[pasted] {} ({:.1} KB)",
                        path.display(),
                        size as f64 / 1024.0
                    ));
                    // Brief status-bar hint.
                    state.run_state.status = format!("pasted {:.1} KB", size as f64 / 1024.0);
                }
                Ok(None) => {}
                Err(e) => {
                    state.push_assistant(format!("[paste error] {e}"));
                    state.run_state.status = format!("paste error");
                }
            }
        }
        KeyAction::Newline => {
            // v0.10.1 (M1): same as Insert — a multi-line prompt is being
            // composed, not submitted.
            state.input.insert_char('\n');
            state.refresh_completion();
        }
        KeyAction::Backspace => {
            // F019: search-mode backspace mutates the query, not the
            // input buffer.
            if state.ui_state.search.is_some() {
                if let Some(search) = state.ui_state.search.as_mut() {
                    search.query.pop();
                }
                let q = state
                    .ui_state
                    .search
                    .as_ref()
                    .map(|s| s.query.clone())
                    .unwrap_or_default();
                state.update_search_query(q);
                return;
            }
            state.input.backspace();
            state.refresh_completion();
        }
        KeyAction::Delete => {
            state.input.delete();
            state.refresh_completion();
        }
        KeyAction::MoveLeft => {
            if state.ui_state.completion.is_some() {
                state.ui_state.completion.as_mut().unwrap().select_up();
            } else {
                state.input.move_left();
            }
        }
        KeyAction::MoveRight => {
            if state.ui_state.completion.is_some() {
                state.ui_state.completion.as_mut().unwrap().select_down();
            } else {
                state.input.move_right();
            }
        }
        KeyAction::MoveLineStart => state.input.move_to_start(),
        KeyAction::MoveLineEnd => state.input.move_to_end(),
        KeyAction::MoveWordLeft => state.input.move_word_left(),
        KeyAction::MoveWordRight => state.input.move_word_right(),
        // v0.10.2: Pi's three-step ladder — see `move_prompt_vertically`.
        KeyAction::MoveUp => {
            if state.ui_state.completion.is_some() {
                state.ui_state.completion.as_mut().unwrap().select_up();
            } else {
                move_prompt_vertically(&mut state, -1);
            }
        }
        KeyAction::MoveDown => {
            if state.ui_state.completion.is_some() {
                state.ui_state.completion.as_mut().unwrap().select_down();
            } else {
                move_prompt_vertically(&mut state, 1);
            }
        }
        KeyAction::KillToLineStart => state.input.kill_to_line_start(),
        KeyAction::KillToLineEnd => state.input.kill_to_line_end(),
        KeyAction::KillWordBackward => state.input.kill_word_backward(),
        KeyAction::KillWordForward => state.input.kill_word_forward(),
        KeyAction::Yank => {
            if !state.input.yank() {
                state.run_state.status = "(kill ring empty)".to_string();
            }
        }
        KeyAction::YankPop => {
            if !state.input.yank_pop() {
                state.run_state.status = "(no previous yank)".to_string();
            }
        }
        KeyAction::Undo => {
            if !state.input.undo() {
                state.run_state.status = "(nothing to undo)".to_string();
            }
        }
        KeyAction::ClearInput => state.input.clear(),
        KeyAction::AcceptCompletionOrInsertTab => {
            if state.ui_state.completion.is_some() {
                state.apply_completion();
            } else if state.run_state.mode == RunMode::Editing {
                // No popup: insert literal tab.
                state.input.insert_char('\t');
            }
        }
        KeyAction::Submit => {
            // v0.10.1 (M1): while a turn is running, Enter queues instead of
            // spawning a second agent. Two agents sharing one
            // `SharedState` and one session file is not what the user meant,
            // and Pi steers the message into the running turn instead.
            if state.run_state.mode == RunMode::Running {
                if state.ui_state.completion.is_some() {
                    state.apply_completion();
                } else {
                    let text = state.input.submit();
                    if !text.trim().is_empty() {
                        queue_while_running(&mut state, &text);
                    }
                }
                return;
            }
            // v0.6: smarter Enter behavior when the completion popup is
            // open. v0.5 always just inserted the highlighted completion
            // into the buffer, requiring a second Enter to actually
            // submit. Now we:
            //   * If exactly one item matches (or the typed prefix is
            //     already exact): submit immediately.
            //   * If multiple candidates AND the highlighted item carries
            //     an `argument_hint`: insert the full command + space
            //     and leave the cursor for arguments (don't submit yet).
            //   * If multiple candidates AND no hint: insert + submit in
            //     one shot.
            //
            // Tab still does the pure "accept" path, so power users can
            // always preview before submitting.
            if let Some(popup) = state.ui_state.completion.as_ref() {
                let items = &popup.items;
                let unique = items.len() == 1;
                let exact = state
                    .input
                    .text
                    .trim_start()
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .map(|cmd| items.iter().any(|i| i.name == cmd))
                    .unwrap_or(false);
                if unique || exact {
                    // Fall through to submit below — but clear the popup
                    // first so submit_user_input sees Editing without
                    // completion.
                    state.ui_state.completion = None;
                } else if popup.selected_item_has_argument_hint() {
                    state.apply_completion();
                    return;
                } else {
                    state.apply_completion();
                    // Don't return — let submit proceed.
                    state.ui_state.completion = None;
                }
            }
            if state.ui_state.completion.is_some() {
                state.apply_completion();
            } else {
                // Install an abort signal for the upcoming agent turn. The
                // agent driver (CLI / wiring code) doesn't have direct
                // access to AppState today; the CLI would need to set this
                // before spawning the driver. v1 stores the signal here so
                // that any future driver-side wiring can read it.
                state.run_state.abort_signal = Some(Arc::new(tokio::sync::Notify::new()));
                drop(state);
                submit_user_input(shared, agent_driver, done, event_tx.clone());
            }
        }
        KeyAction::Abort => {
            if state.ui_state.completion.is_some() {
                state.ui_state.completion = None;
            } else if state.ui_state.search.is_some() {
                // Esc exits transcript search (F019).
                state.end_search();
                state.run_state.status = "ready".to_string();
            } else if state.run_state.pending_quit.is_some() {
                // Esc cancels the pending Ctrl+D quit confirmation.
                state.run_state.pending_quit = None;
                state.run_state.status = "ready".to_string();
            } else if state.run_state.mode == RunMode::Running {
                // Signal the agent to abort, then hand the editor back.
                //
                // v0.10.1 (M1): the queue goes back to the editor FIRST —
                // Pi: `restoreQueuedMessagesToEditor({ abort: true })`
                // merges queued messages with what is being typed and
                // returns, then aborts (`interactive-mode.js:3505`). Doing
                // it in that order means an interrupt never destroys
                // something the user wrote and had queued.
                let restored = return_queue_to_editor(&mut state);
                if let Some(sig) = state.run_state.abort_signal.as_ref() {
                    sig.notify_waiters();
                }
                if restored > 0 {
                    state.run_state.status =
                        format!("aborted — {restored} queued message(s) returned to the editor");
                }
                // v0.10.1 (fix): this used to set `RunMode::Aborted`, a state
                // Pi does not have. Every input path in this file gates on
                // `mode == Editing`, and nothing set `Aborted` back, so the
                // keys typed after an interrupt were dropped until the agent
                // stream finished on its own and pushed `Done` — measured in
                // tmux at roughly four seconds against a slow provider.
                //
                // Pi's interrupt is synchronous: `onEscape` aborts and calls
                // `restoreQueuedMessagesToEditor({ abort: true })`, so the
                // editor is usable the instant you press Esc. The `aborted`
                // label stays, as `run_state.status`, so the user still sees
                // that the turn was cut short.
                state.run_state.mode = RunMode::Editing;
                state.run_state.status = "aborted".to_string();
            } else {
                state.input.clear();
            }
        }
        KeyAction::Quit => {
            // v0.6: Ctrl+D requires double-tap to actually quit. The
            // first press sets a pending flag + a status message; the
            // second press within `QUIT_CONFIRM_WINDOW_MS` exits. After
            // the window elapses without confirmation, the pending flag
            // clears and Ctrl+D is a no-op again. This prevents losing
            // a half-written prompt or in-flight work to a stray Ctrl+D.
            use std::time::{Duration, Instant};
            const QUIT_CONFIRM_WINDOW_MS: u64 = 3000;
            let now = Instant::now();
            if let Some(pending_at) = state.run_state.pending_quit {
                if now.duration_since(pending_at) <= Duration::from_millis(QUIT_CONFIRM_WINDOW_MS) {
                    state.run_state.pending_quit = None;
                    state.run_state.mode = RunMode::Quitting;
                } else {
                    // Window expired; treat this as the first tap again.
                    state.run_state.pending_quit = Some(now);
                    state.run_state.status =
                        "Press Ctrl+D again to quit, or Esc to cancel".to_string();
                }
            } else {
                state.run_state.pending_quit = Some(now);
                state.run_state.status = "Press Ctrl+D again to quit, or Esc to cancel".to_string();
            }
        }
        KeyAction::SwitchModel => {
            // Mirror `/model` (no args) — set the status flag so the main
            // loop's "open_selector:model" branch spins up the selector.
            // The previous implementation only set a status string, which
            // made Ctrl+L a no-op despite the CHANGELOG claiming
            // ModelSelector is wired.
            state.run_state.status = "open_selector:model".to_string();
        }
        KeyAction::OpenExternalEditor => {
            // F020: signal the run loop to suspend the TUI, spawn
            // $VISUAL/$EDITOR on the current input, then resume.
            // We just set the flag here; the loop owns the actual
            // suspend/resume dance because it has access to the
            // terminal handle. The flag is checked on the next
            // event-loop iteration.
            state.run_state.pending_external_editor = true;
            state.run_state.status = "opening editor…".to_string();
        }
        KeyAction::CycleModelNext => cycle_model(&mut state, 1),
        KeyAction::CycleModelPrev => cycle_model(&mut state, -1),
        KeyAction::CycleThinkingNext => cycle_thinking(&mut state, 1),
        KeyAction::CycleThinkingPrev => cycle_thinking(&mut state, -1),
        KeyAction::ShowHelp => {
            // v0.6: F1 toggles the help overlay + extended footer.
            // The overlay-close half is handled in the main loop via
            // `state.ui_state.close_help` (set true here; loop clears the
            // active selector and resets the flag).
            if state.ui_state.help_extended {
                state.ui_state.help_extended = false;
                state.ui_state.close_help = true;
                state.run_state.status = "ready".to_string();
            } else {
                state.ui_state.help_extended = true;
                state.run_state.status = "open_selector:help".to_string();
            }
        }
        // v0.10.2: shared with `apply_action` — see `scroll_transcript`.
        KeyAction::ScrollUp => scroll_transcript(&mut state, 1),
        KeyAction::ToggleCollapse => {
            // Toggle collapsed on the most-recent collapsible transcript
            // line.
            //
            // The comment here used to add: "The user can scroll first
            // (PageUp/PageDown) to put a specific line at the top, then
            // press Ctrl+O." That interaction does not exist — the code
            // below starts at `total - 1` and `scroll_offset` is never
            // read. It described a feature and sat next to code that did
            // the opposite, which is worse than no comment. Either the
            // scroll-aware behaviour gets written or the claim stays gone;
            // as of v0.10 it is not implemented and the test below says so.
            let total = state.transcript_len();
            if total > 0 {
                let mut idx = total - 1;
                loop {
                    if state.toggle_collapsed(idx) {
                        break;
                    }
                    if idx == 0 {
                        break;
                    }
                    idx -= 1;
                }
            }
        }
        KeyAction::ScrollDown => scroll_transcript(&mut state, -1),
        KeyAction::ScrollToBottom => scroll_to_bottom(&mut state),
        KeyAction::ScrollToTop => scroll_to_top(&mut state),
        KeyAction::Noop => {}
    }
}

// ---------------------------------------------------------------------------
// Mouse
// ---------------------------------------------------------------------------

/// A scrollbar drag in progress.
///
/// Pi's `ScrollbarDrag` (`tui-alt-screen.ts:114-117`) is exactly these two
/// fields: which scroll view, and how far below the thumb's top the pointer
/// grabbed it. The grab offset is what makes a drag feel attached rather than
/// teleporting - the thumb keeps the same point under the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarDrag {
    /// `event.y - geometry.thumbTop` at the moment of the press.
    /// Signed, because Pi's is (`event.y - geometry.thumbTop`,
    /// `tui-alt-screen.ts:777`) and a press *above* the thumb gives a
    /// negative offset. The first version used `u16` with a saturating
    /// subtraction, which silently turned "grabbed two rows above the thumb"
    /// into "grabbed the thumb's top" — the pointer then jumps the thumb up
    /// by two rows the instant the drag starts. Found by a test that was
    /// itself wrong for an unrelated reason and had to be fixed first.
    pub grab_offset: i32,
}

/// Handle one mouse event.
///
/// Returns whether it was consumed. Everything here is deliberately
/// narrow: wheel, scrollbar drag, and hover. Pi also does text selection
/// and right-click paste, and nini does not yet - see the plan.
pub fn handle_mouse(m: crossterm::event::MouseEvent, shared: &SharedState) -> bool {
    use crossterm::event::{MouseButton, MouseEventKind};

    let mut state = lock_state(shared);

    // `Moved` arrives for every pointer motion because crossterm enables
    // 1003. Pi leaves that mode off. Nothing here acts on motion alone, so
    // it is dropped rather than forcing a redraw.
    if matches!(m.kind, MouseEventKind::Moved) {
        return false;
    }

    // Overlays own the pointer while they are up: a wheel event inside a
    // selector scrolls the list, not the transcript behind it.
    if state.ui_state.selector.is_some() || state.ui_state.completion.is_some() {
        return false;
    }

    let current_offset = state.transcript_state.scroll_offset;

    match m.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let up = matches!(m.kind, MouseEventKind::ScrollUp);
            let step = crate::scrollbar::wheel_lines();
            let max_offset = state.transcript_state.lines.len().saturating_sub(1);
            let next = if up {
                current_offset.saturating_add(step).min(max_offset)
            } else {
                current_offset.saturating_sub(step)
            };
            if next != current_offset {
                Arc::make_mut(&mut state.transcript_state).scroll_offset = next;
                Arc::make_mut(&mut state.transcript_state).autoscroll = next == 0;
                state.mark_scroll_activity();
                return true;
            }
            false
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let vp = match transcript_viewport(&state) {
                Some(v) => v,
                None => return false,
            };
            let Some(g) = vp.geometry(current_offset) else {
                return false;
            };
            if !vp.hits_column(m.column, m.row) {
                return false;
            }
            // Grabbing the rail above or below the thumb starts a drag too,
            // which is how a single press jumps. The thumb then snaps to the
            // pointer minus this offset.
            let grab_offset = m.row as i32 - g.thumb_top as i32;
            state.ui_state.scrollbar_drag = Some(ScrollbarDrag { grab_offset });
            state.mark_scroll_activity();
            true
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            let Some(drag) = state.ui_state.scrollbar_drag else {
                return false;
            };
            let vp = match transcript_viewport(&state) {
                Some(v) => v,
                None => return false,
            };
            let Some(g) = vp.geometry(current_offset) else {
                return false;
            };
            let heights = state
                .line_heights
                .lock()
                .map(|h| h.clone())
                .unwrap_or_default();
            let thumb_offset = (m.row as i64 - vp.y as i64 - drag.grab_offset as i64)
                .clamp(0, i64::from(u16::MAX)) as usize;
            if let Some(next) =
                crate::scrollbar::line_offset_for_thumb_offset(&heights, thumb_offset, &g)
            {
                let max_offset = state.transcript_state.lines.len().saturating_sub(1);
                let next = next.min(max_offset);
                if next != current_offset {
                    Arc::make_mut(&mut state.transcript_state).scroll_offset = next;
                    Arc::make_mut(&mut state.transcript_state).autoscroll = next == 0;
                    state.mark_scroll_activity();
                }
            }
            true
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if state.ui_state.scrollbar_drag.take().is_some() {
                state.mark_scroll_activity();
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

/// The transcript's on-screen rectangle, as last drawn.
///
/// The render pass is the only place that knows it, so it is published
/// alongside `line_heights`. Without a prior frame there is nothing to hit
/// and every mouse event is a no-op, which is the correct answer for the
/// first few milliseconds of startup.
pub fn transcript_viewport(state: &AppState) -> Option<crate::scrollbar::Viewport> {
    state.transcript_viewport.lock().ok().map(|v| *v)
}

/// Submit handler: extract text, intercept slash commands, otherwise spawn agent.
///
/// Slash commands are dispatched locally — they run synchronously and do NOT
/// transition the TUI to Running mode. Non-slash input goes to the agent
/// driver as before.
pub fn submit_user_input(
    shared: &SharedState,
    agent_driver: &AgentDriver,
    done: Arc<Notify>,
    event_tx: tokio::sync::watch::Sender<bool>,
) {
    // v0.8.5: caller (the runtime loop) passes the SHARED watch sender
    // so all agent tasks created during this runtime lifetime wake
    // the same loop subscription. Replaces v0.7.4's per-call fresh
    // `Arc::new(Notify::new())` that was never connected to the
    // runtime's select!.
    submit_user_input_inner(shared, agent_driver, done, event_tx)
}

/// Internal helper for `submit_user_input` — takes an additional
/// `event_tx` arg that is passed to the agent sink so the runtime
/// can wake on each event arrival.
fn submit_user_input_inner(
    shared: &SharedState,
    agent_driver: &AgentDriver,
    done: Arc<Notify>,
    event_tx: tokio::sync::watch::Sender<bool>,
) {
    let text = {
        let mut g = lock_state(&shared);
        if g.run_state.mode != RunMode::Editing {
            return;
        }
        let text = g.input.submit();
        if text.trim().is_empty() {
            return;
        }

        // ── Bash passthrough (!cmd, !!cmd) ──────────────────────────────────
        // `!cmd` executes locally and pushes a BashExecution transcript line.
        // `!!cmd` does the same but does NOT also inject the output into the
        // session context for the agent.
        if let Some(rest) = text.strip_prefix('!') {
            // v0.6: `!!cmd` is a privacy marker — runs the command and
            // shows the output in the transcript, but does NOT inject
            // it into the agent's session context. Useful for sensitive
            // commands (`!!cat ~/.aws/credentials`) where the user
            // wants the LLM to never see the bytes.
            let (private, cmd) = match rest.strip_prefix('!') {
                Some(c) => (true, c.trim()),
                None => (false, rest.trim()),
            };
            if !cmd.is_empty() {
                let mut cx = nini_tools::bash_runner::BashRunner::new();
                let result = cx.run_blocking(cmd, &std::env::current_dir().unwrap_or_default());
                use crate::state::TranscriptLine;
                let id = format!("bash-{}", g.transcript_len());
                // Strip ANSI codes + truncate so the transcript stays clean.
                let cleaned_out = crate::ansi::strip_ansi(&result.output);
                let (output, _truncated, _ob, _ol) =
                    crate::ansi::truncate(&cleaned_out, 16 * 1024, 200);
                // For timeout, append a clear suffix so users know output
                // may be incomplete.
                let output = if result.timed_out {
                    format!("{output}\n…(timed out after {} ms)", result.duration_ms)
                } else {
                    output
                };
                // Show `!!cmd` in the transcript so the user can see what was
                // actually executed (the privacy marker is for the agent,
                // not for the user).
                let cmd_display = if private {
                    format!("!!{cmd}")
                } else {
                    cmd.to_string()
                };
                Arc::make_mut(&mut g.transcript_state)
                    .lines
                    .push(TranscriptLine::BashExecution {
                        id,
                        cmd: cmd_display,
                        output: output.clone(),
                        stderr: crate::ansi::strip_ansi(&result.stderr),
                        ok: result.ok,
                        exit_code: result.exit_code,
                        duration_ms: result.duration_ms,
                        collapsed: false,
                    });
                // v0.10 (R2f): record it too. The transcript line above is
                // display only - without a session entry the command is gone
                // on `/resume`, and `render_context_entries` rebuilds from
                // the session rather than from what is on screen.
                g.session_append_bash_execution(
                    &cmd,
                    &output,
                    &crate::ansi::strip_ansi(&result.stderr),
                    result.ok,
                    result.exit_code,
                    result.duration_ms,
                );
                g.session_flush();
                g.push_divider();
                // For `!cmd`, also inject the output as a user message so
                // the agent sees it on the next turn. For `!!cmd` the
                // privacy marker tells us NOT to do that — the LLM will
                // never learn the contents of this command.
                if !private {
                    g.run_state.pending_next_turn_messages.push(format!(
                        "[bash $ {}]\n{}",
                        cmd,
                        if result.stderr.is_empty() {
                            output.clone()
                        } else {
                            format!("{output}\n[stderr]\n{}", result.stderr)
                        }
                    ));
                }
                // Brief status bar hint so users can confirm the privacy
                // marker actually fired.
                if private {
                    g.run_state.status = "executed (private)".to_string();
                }
                return;
            }
        }

        // ── Slash-command interception ──────────────────────────────────────
        // Mirror the same dispatch logic as apply_action (tests) so the live
        // TUI and tests share one code path.
        if let Some((cmd_id, args)) = crate::commands::parse(&text) {
            // Special-case: /model, /thinking, /session, /tree, /trust open
            // interactive selectors when invoked WITHOUT arguments.
            use crate::commands::CommandId;
            match cmd_id {
                CommandId::Model if args.trim().is_empty() => {
                    g.push_assistant("(opening model selector...)".to_string());
                    g.push_divider();
                    g.run_state.status = "open_selector:model".to_string();
                    let text_for_agent = text.clone();
                    return drop_and_signal(text_for_agent, done);
                }
                CommandId::Thinking if args.trim().is_empty() => {
                    g.push_assistant("(opening thinking selector...)".to_string());
                    g.push_divider();
                    g.run_state.status = "open_selector:thinking".to_string();
                    let text_for_agent = text.clone();
                    return drop_and_signal(text_for_agent, done);
                }
                CommandId::Session if args.trim().is_empty() => {
                    g.push_assistant("(opening session selector...)".to_string());
                    g.push_divider();
                    g.run_state.status = "open_selector:session".to_string();
                    let text_for_agent = text.clone();
                    return drop_and_signal(text_for_agent, done);
                }
                CommandId::Tree if args.trim().is_empty() => {
                    g.push_assistant("(opening tree selector...)".to_string());
                    g.push_divider();
                    g.run_state.status = "open_selector:tree".to_string();
                    let text_for_agent = text.clone();
                    return drop_and_signal(text_for_agent, done);
                }
                CommandId::Trust if args.trim().is_empty() => {
                    g.push_assistant("(opening trust selector...)".to_string());
                    g.push_divider();
                    g.run_state.status = "open_selector:trust".to_string();
                    let text_for_agent = text.clone();
                    return drop_and_signal(text_for_agent, done);
                }
                CommandId::Settings if args.trim().is_empty() => {
                    g.push_assistant("(opening settings selector...)".to_string());
                    g.push_divider();
                    g.run_state.status = "open_selector:settings".to_string();
                    let text_for_agent = text.clone();
                    return drop_and_signal(text_for_agent, done);
                }
                _ => {}
            }
            // SettingsManager is created per-call so concurrent turns don't share state.
            let mut settings = crate::settings::SettingsManager::default();
            // Dispatch locally; slash commands are synchronous — no Running mode.
            let result = crate::commands::dispatch(&mut g, &mut settings, cmd_id, &args);
            // Error from dispatch: show error in transcript and stop (do NOT fall
            // through to agent). Matches pi's try/catch per-command pattern.
            if let Some(err) = result.error {
                g.push_assistant(format!("[command error] {err}"));
                g.push_divider();
                return;
            }
            match result.outcome {
                crate::commands::CommandOutcome::Output(lines) => {
                    for line in lines {
                        g.push_assistant(line);
                    }
                    g.push_divider();
                }
                crate::commands::CommandOutcome::Quit => {
                    g.run_state.mode = RunMode::Quitting;
                }
                crate::commands::CommandOutcome::PromptArgument { prompt, next: _ } => {
                    // v1: prompt-argument flow not wired; show fallback.
                    g.push_assistant(format!("(prompt: {prompt})"));
                    g.push_divider();
                }
            }
            return; // Slash commands are fully handled — do NOT spawn agent.
        }

        // ── Regular user message: push to transcript and spawn agent ───────
        // Pi parity: if compaction is in progress, queue the message
        // instead of spawning an agent. The queued messages are injected
        // as context alongside the next user prompt (mirrors Pi's
        // _pendingNextTurnMessages).
        if g.run_state.is_compacting {
            let n = g.run_state.pending_next_turn_messages.len() + 1;
            g.run_state.pending_next_turn_messages.push(text.clone());
            // Use explicit let-bindings to avoid `format!` holding
            // simultaneous borrows on `g`.
            let msg = format!(
                "(queued: {} message{} pending; will run after compaction)",
                n,
                if n == 1 { "" } else { "s" }
            );
            g.push_assistant(msg);
            g.push_divider();
            return;
        }

        // Pi parity: flush any branch summaries or compaction-time queued
        // messages into the transcript as User-content (so the agent sees
        // them as context asides on the next turn). Then clear the queue.
        let queued = std::mem::take(&mut g.run_state.pending_next_turn_messages);
        for aside in &queued {
            g.push_user(aside.clone());
            g.push_divider();
        }

        // ── Auto-compaction trigger ───────────────────────────────────────
        // Before dispatching the user message, check whether the transcript
        // is over budget. If so, fold the older half into a deterministic
        // summary. This is the local-heuristic fallback path; when an
        // LLM provider is wired in (via the agent driver), the driver
        // also calls `should_auto_compact` and produces an LLM summary
        // before the model request, so this serves as the offline
        // default.
        // A2: this used to call `auto_compact_local`, which folded the
        // first half of the *transcript lines* at `total / 2` and wrote
        // nothing anywhere - the model never saw a summary and a restart
        // brought the whole prefix back. The cut now comes from the token
        // budget and the result is a real CompactionEntry.
        if g.should_auto_compact(&g.model_state.settings_snapshot) {
            let keep = g.model_state.settings_snapshot.keep_recent_tokens;
            match g.session_compact(keep) {
                Some(r) => {
                    g.session_flush();
                    g.push_divider();
                    g.push_assistant(format!(
                        "[auto-compact] folded {} entries into a summary \
                         ({} tokens before)",
                        r.folded, r.tokens_before
                    ));
                }
                None => {
                    g.push_assistant("[auto-compact] nothing to compact".to_string());
                }
            }
        }

        g.push_user(text.clone());
        g.push_divider();
        g.run_state.mode = RunMode::Running;
        g.run_state.status = "running...".to_string();
        text
    };

    // v0.10 (R2a/A3): hand the driver the session's live context.
    //
    // This is the point where the session is reachable, so it is where
    // `build_context_entries` belongs. The Agent used to start empty every
    // run while the session held the whole history — two stores, one of
    // which the model never saw. A compaction recorded in the session is
    // therefore visible to the very next turn, which is the entire reason
    // P0.1, P0.2 and A1 were worth doing.
    let seed: Vec<nini_core::provider::Message> = {
        let g = lock_state(&shared);
        g.session_context_messages()
    };

    // Spawn the agent task with its own sink.
    let sink = AgentSink::new(shared.clone(), event_tx.clone());
    let _handle = (agent_driver)(text, seed, sink, done.clone());
    // The handle is intentionally dropped — the task continues running in
    // the background. We don't abort the agent on quit; the runtime owns
    // the SharedState and the sink keeps a clone.
}

/// Apply a `KeyAction` to a state without spawning anything. Used by tests
/// and the no-agent mode.
/// The prompt's inner width, as published by `render_prompt`.
/// Pi's `Up` / `Down` ladder (`editor.ts:833-857`).
///
/// v0.10.2. Both keys used to call `recall_history(±1)` unconditionally,
/// so they were *only* a history browser — while `render_prompt` draws
/// the prompt as several rows. The user saw a three-row draft, pressed
/// Up expecting the caret to rise, and the entire prompt was replaced by
/// an earlier message. Measured: one keypress, the whole draft gone.
///
/// ```text
///   Up   : first row && (empty || browsing history || column 0)
///             -> navigateHistory(-1)
///          first row                        -> moveToLineStart()
///          otherwise                        -> moveCursor(-1, 0)
///
///   Down : browsing history && last row    -> navigateHistory(1)
///          last row                         -> moveToLineEnd()
///          otherwise                        -> moveCursor(1, 0)
/// ```
///
/// One function, not one per call site: `handle_key` and `apply_action`
/// each carried their own copy of this match, which is precisely how the
/// keymap ended up in three divergent copies (footer, `/hotkeys`,
/// README) — see the CHANGELOG entry for 2026-10-17.
fn move_prompt_vertically(state: &mut AppState, delta: i32) {
    // `prompt_inner_width` is published by `render_prompt`. 0 means no
    // frame has been drawn yet, and `visual_rows_of` reads that as "rows
    // are `\n`-separated" — every single-line prompt then behaves exactly
    // as it did before.
    let w = prompt_width(state);
    let (row, col) = state.input.visual_position(w);
    let rows = state.input.visual_row_count(w).max(1);
    let browsing = state.input.history_cursor.is_some();

    if delta < 0 {
        if row == 0 && (state.input.text.is_empty() || browsing || col == 0) {
            state.input.recall_history(-1);
        } else if row == 0 {
            state.input.move_to_line_start();
        } else {
            state.input.move_visual_row(-1, w);
        }
    } else if browsing && row + 1 >= rows {
        state.input.recall_history(1);
    } else if row + 1 >= rows {
        state.input.move_to_line_end();
    } else {
        state.input.move_visual_row(1, w);
    }
}

// ---- v0.10.2: transcript scrolling ------------------------------------
//
// One implementation, two dispatch sites. `handle_key` and `apply_action`
// each carried their own copy of the scrolling match, and `apply_action`'s
// copy was **entirely empty**:
//
//     KeyAction::ScrollUp | ScrollDown | ScrollToBottom => {}
//
// with the comment "this branch exists so the second key-dispatch site is
// exhaustive". It has no production caller today — only tests — but that
// is exactly how P0-5's `Up`/`Down` went unfixed for three releases while
// the tests kept passing, and how the keymap ended up in three divergent
// copies on the same day. A dispatch point that silently drops half the
// actions is a trap, not a convenience.

/// One page up or down. `page_lines` is published by the renderer, the
/// only scope holding both the per-line row counts and the viewport
/// height; guessing `1` was the pre-v0.10 bug, because `scroll_offset`
/// counts TranscriptLines and one of those can render across fifty rows.
fn scroll_transcript(state: &mut AppState, delta: i32) {
    let max_offset = state.transcript_state.lines.len().saturating_sub(1) as i32;
    let step = state.page_lines() as i32;
    let current = state.transcript_state.scroll_offset as i32;
    let next = (current + delta * step).clamp(0, max_offset) as usize;
    let ts = Arc::make_mut(&mut state.transcript_state);
    ts.scroll_offset = next;
    ts.autoscroll = next == 0;
    state.mark_scroll_activity();
}

/// `home` — `tui.altScreen.top` (`keybindings.ts:208`).
///
/// `scroll_offset` counts TranscriptLines skipped **from the bottom**, so
/// the oldest line on screen is `total - 1`. `autoscroll` must come off:
/// the renderer's tail-aligned branch ignores `scroll_offset` entirely.
fn scroll_to_top(state: &mut AppState) {
    let total = state.transcript_state.lines.len();
    let ts = Arc::make_mut(&mut state.transcript_state);
    ts.autoscroll = false;
    ts.scroll_offset = total.saturating_sub(1);
    state.mark_scroll_activity();
}

/// `end` — `tui.altScreen.bottom` (`keybindings.ts:209`).
///
/// Re-enabling autoscroll is the point: it is what makes subsequent
/// streaming tokens stick to the bottom again.
fn scroll_to_bottom(state: &mut AppState) {
    let ts = Arc::make_mut(&mut state.transcript_state);
    ts.scroll_offset = 0;
    ts.autoscroll = true;
    state.mark_scroll_activity();
}

fn prompt_width(state: &AppState) -> usize {
    state
        .prompt_inner_width
        .load(std::sync::atomic::Ordering::Relaxed)
}

pub fn apply_action(state: &mut AppState, key: Key) {
    let action = resolve_with_user_overrides(key);
    match action {
        KeyAction::Insert(c) => {
            // v0.10.1 (M1): same reasoning as the runtime path.
            state.input.insert_char(c);
            state.refresh_completion();
        }
        KeyAction::OpenCommandPalette => {
            // Test path: same as the runtime path (open_selector:palette).
            if state.run_state.mode == RunMode::Editing {
                state.ui_state.completion = None;
                state.run_state.status = "open_selector:palette".to_string();
            }
        }
        KeyAction::OpenSearch => {
            // Test path mirrors the runtime path.
            if state.run_state.mode == RunMode::Editing
                && state.ui_state.completion.is_none()
                && state.input.text.is_empty()
            {
                state.begin_search();
            } else if state.run_state.mode == RunMode::Editing {
                state.input.insert_char('/');
                state.refresh_completion();
            }
        }
        KeyAction::OpenExternalEditor => {
            // F020: same as the runtime path — set the flag and
            // let the run loop handle the suspend/resume dance.
            state.run_state.pending_external_editor = true;
            state.run_state.status = "opening editor…".to_string();
        }
        KeyAction::PasteImage => {
            // Try to read an image from the system clipboard. If found,
            // insert its `[pasted image: <path>]` description into the
            // prompt AND surface a short transcript confirmation so
            // the user sees what landed. If the clipboard has no image
            // (or is unavailable in headless env), silently fall
            // through to Noop.
            match crate::image_paste::paste_image_with_size_from_clipboard() {
                Ok(Some((path, size))) => {
                    let desc = crate::image_paste::describe_path(&path);
                    // Insert the description char-by-char into the
                    // prompt input.
                    for c in desc.chars() {
                        state.input.insert_char(c);
                    }
                    state.refresh_completion();
                    // Surface a short echo in the transcript.
                    state.push_assistant(format!(
                        "[pasted] {} ({:.1} KB)",
                        path.display(),
                        size as f64 / 1024.0
                    ));
                    // Brief status-bar hint.
                    state.run_state.status = format!("pasted {:.1} KB", size as f64 / 1024.0);
                }
                Ok(None) => {}
                Err(e) => {
                    state.push_assistant(format!("[paste error] {e}"));
                    state.run_state.status = format!("paste error");
                }
            }
        }
        KeyAction::Newline => {
            // v0.10.1 (M1): same as Insert — a multi-line prompt is being
            // composed, not submitted.
            state.input.insert_char('\n');
            state.refresh_completion();
        }
        KeyAction::Backspace => {
            // F019: search-mode backspace mutates the query, not the
            // input buffer.
            if state.ui_state.search.is_some() {
                if let Some(search) = state.ui_state.search.as_mut() {
                    search.query.pop();
                }
                let q = state
                    .ui_state
                    .search
                    .as_ref()
                    .map(|s| s.query.clone())
                    .unwrap_or_default();
                state.update_search_query(q);
                return;
            }
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
            if state.ui_state.completion.is_some() {
                if state.ui_state.completion.as_ref().unwrap().selected == 0 {
                    // wrap
                } else {
                    state.ui_state.completion.as_mut().unwrap().select_up();
                }
            } else {
                state.input.move_left();
            }
        }
        KeyAction::MoveRight => {
            if state.ui_state.completion.is_some() {
                state.ui_state.completion.as_mut().unwrap().select_down();
            } else {
                state.input.move_right();
            }
        }
        KeyAction::MoveLineStart => state.input.move_to_start(),
        KeyAction::MoveLineEnd => state.input.move_to_end(),
        KeyAction::MoveWordLeft => state.input.move_word_left(),
        KeyAction::MoveWordRight => state.input.move_word_right(),
        KeyAction::MoveUp => {
            if state.ui_state.completion.is_some() {
                state.ui_state.completion.as_mut().unwrap().select_up();
            } else {
                move_prompt_vertically(state, -1);
            }
        }
        KeyAction::MoveDown => {
            if state.ui_state.completion.is_some() {
                state.ui_state.completion.as_mut().unwrap().select_down();
            } else {
                move_prompt_vertically(state, 1);
            }
        }
        KeyAction::KillToLineStart => state.input.kill_to_line_start(),
        KeyAction::KillToLineEnd => state.input.kill_to_line_end(),
        KeyAction::KillWordBackward => state.input.kill_word_backward(),
        KeyAction::KillWordForward => state.input.kill_word_forward(),
        KeyAction::Yank => {
            if !state.input.yank() {
                state.run_state.status = "(kill ring empty)".to_string();
            }
        }
        KeyAction::YankPop => {
            if !state.input.yank_pop() {
                state.run_state.status = "(no previous yank)".to_string();
            }
        }
        KeyAction::Undo => {
            if !state.input.undo() {
                state.run_state.status = "(nothing to undo)".to_string();
            }
        }
        KeyAction::ClearInput => state.input.clear(),
        KeyAction::Submit => {
            // v0.10.1 (M1): a turn in flight no longer swallows the
            // message. Pi steers it into the running turn
            // (`interactive-mode.js:2491`); the queue is where nini keeps
            // pending messages.
            if state.run_state.mode == RunMode::Running {
                if state.ui_state.completion.is_some() {
                    state.apply_completion();
                } else {
                    let text = state.input.submit();
                    if !text.trim().is_empty() {
                        queue_while_running(state, &text);
                    }
                }
            } else if state.run_state.mode == RunMode::Editing {
                // If completion popup is showing, accept the selected item
                // instead of submitting.
                if state.ui_state.completion.is_some() {
                    state.apply_completion();
                    return;
                }
                let text = state.input.submit();
                if !text.trim().is_empty() {
                    // Slash command interception
                    if let Some((cmd_id, args)) = crate::commands::parse(&text) {
                        use crate::commands::dispatch;
                        let mut settings = crate::settings::SettingsManager::default();
                        let result = dispatch(state, &mut settings, cmd_id, &args);
                        if let Some(err) = result.error {
                            state.push_assistant(format!("[command error] {err}"));
                            state.push_divider();
                            return;
                        }
                        match result.outcome {
                            crate::commands::CommandOutcome::Output(lines) => {
                                for line in lines {
                                    state.push_assistant(line);
                                }
                                state.push_divider();
                            }
                            crate::commands::CommandOutcome::Quit => {
                                state.run_state.mode = RunMode::Quitting;
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
            if state.ui_state.completion.is_some() {
                // Cancel popup without changing input.
                state.ui_state.completion = None;
            } else if state.run_state.mode == RunMode::Running {
                // v0.10.1: same as the runtime path — the queue returns to
                // the editor, then the turn is interrupted.
                return_queue_to_editor(state);
                state.run_state.mode = RunMode::Editing;
                state.run_state.status = "aborted".to_string();
            } else {
                state.input.clear();
            }
        }
        KeyAction::Quit => state.run_state.mode = RunMode::Quitting,
        KeyAction::SwitchModel | KeyAction::ShowHelp => {}
        // v0.10.2: these used to be no-ops here. See the comment on
        // `scroll_transcript` — a second dispatch point that silently
        // drops half the actions is a trap.
        KeyAction::ScrollUp => scroll_transcript(state, 1),
        KeyAction::ScrollDown => scroll_transcript(state, -1),
        KeyAction::ScrollToBottom => scroll_to_bottom(state),
        KeyAction::ScrollToTop => scroll_to_top(state),
        KeyAction::CycleModelNext => cycle_model(state, 1),
        KeyAction::CycleModelPrev => cycle_model(state, -1),
        KeyAction::CycleThinkingNext => cycle_thinking(state, 1),
        KeyAction::CycleThinkingPrev => cycle_thinking(state, -1),
        KeyAction::ToggleCollapse => {
            // No-op: the primary handler at the top of the loop already
            // performed the toggle. This branch exists so the second
            // key-dispatch site (for event-bus integration) is exhaustive.
        }
        KeyAction::AcceptCompletionOrInsertTab => {}
        KeyAction::Noop => {}
    }
}

// Suppress unused warning for the dropped-join-handle field.
#[allow(dead_code)]
fn _typecheck_handle_is_used() {
    let _ = std::mem::size_of::<JoinHandle<()>>();
}

/// Queue a message submitted while a turn is in flight.
///
/// v0.10.1 (M1). Pi: `await this.session.prompt(text, { streamingBehavior:
/// "steer" })` followed by `updatePendingMessagesDisplay()`
/// (`interactive-mode.js:2491` and `:2500`) — the message goes to the
/// running turn, and the user is told it is pending.
///
/// nini had one queue already, `pending_next_turn_messages`, written only by
/// a compaction branch that can never run (§ `is_compacting` is never set).
/// Rather than add a second field for Pi's steering/follow-up split, the
/// existing queue is used: one queue is what the struct can express, and a
/// field the fix introduces cannot be named by a test that has to compile
/// against the parent commit.
///
/// The transcript line is the "pending" display. It is pushed as a user
/// message so the agent sees it in context, exactly like the compaction
/// queue does today.
pub fn queue_while_running(state: &mut AppState, text: &str) {
    let n = state.run_state.pending_next_turn_messages.len() + 1;
    state
        .run_state
        .pending_next_turn_messages
        .push(text.to_string());
    // The transcript gets the message verbatim. An earlier version appended
    // "(queued: N pending…)" to it, and `push_user` writes the same string
    // to the session — so the model would have been handed the marker as
    // part of the user's own words, once through the session and again
    // through steering. The pending count belongs in the status line.
    state.push_user(text);
    state.push_divider();
    state.run_state.status = format!("queued: {n} pending");
    state.mark_scroll_activity();
}

/// Hand the queue back to the editor, then abort.
///
/// Pi: `restoreQueuedMessagesToEditor({ abort: true })`
/// (`interactive-mode.js:3505`) — queues are drained, merged with whatever
/// is in the editor, and put back so nothing the user typed is lost when a
/// turn is interrupted.
pub fn return_queue_to_editor(state: &mut AppState) -> usize {
    let queued = std::mem::take(&mut state.run_state.pending_next_turn_messages);
    if queued.is_empty() {
        return 0;
    }
    let queued_text = queued.join("\n\n");
    let combined = [queued_text, state.input.text.clone()]
        .into_iter()
        .filter(|t| !t.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    state.input.text = combined;
    // Caret at the end, which is where a restored queue is useful: the user
    // edits what comes back.
    state.input.cursor = state.input.text.len();
    state.refresh_completion();
    queued.len()
}

/// Resolve a key against the built-in keymap plus any user overrides
/// from `~/.nini/agent/keybindings.json`. The override file is loaded
/// once on first use and cached for the process lifetime.
pub fn resolve_with_user_overrides(key: Key) -> KeyAction {
    use crate::keybindings_manager::load_user_overrides;
    use std::sync::OnceLock;

    static OVERRIDES: OnceLock<Vec<(KeyAction, Key)>> = OnceLock::new();
    let overrides = OVERRIDES.get_or_init(load_user_overrides);

    // User overrides take priority over built-ins.
    for (action, k) in overrides {
        if *k == key {
            return *action;
        }
    }
    resolve(&crate::keys::default_keymap(), key)
}
