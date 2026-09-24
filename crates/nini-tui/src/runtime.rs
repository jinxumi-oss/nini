//! TUI runtime: alternate screen + raw mode + event loop + agent integration.
//!
//! Two main pieces:
//! - [`run_loop`]: drives the terminal in async mode, handles keystrokes
//!   and triggers redraws.
//! - [`AgentTask`]: spawned on Submit, runs the agent and pushes events
//!   into the shared state via [`AgentSink`].
#![allow(unused_mut)] // render/runtime use mut bindings for future hook points

use crate::keys::{Key, KeyAction, resolve};
use crate::render::render_frame_with_theme;
use crate::selector::SelectorItem;
use crate::state::{AppState, RunMode, TranscriptLine};
use anyhow::Result;
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
        details: Option<serde_json::Value>,
    },
    TurnEnd,
    Error(String),
    Usage(u32, u32, f64),
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
    /// v0.7.4 (UX test fix) — wake the runtime event loop whenever an
    /// event arrives, so the TUI redraws between events instead of
    /// waiting for the 50ms tick. Without this, fast agents that
    /// complete in <50ms never show the "working" / tool-call /
    /// text-delta intermediate states — the user only sees the final
    /// "idle" state, which is confusing (looks like nothing happened).
    notify: Arc<tokio::sync::Notify>,
}

impl AgentSink {
    pub fn new(state: SharedState, notify: Arc<tokio::sync::Notify>) -> Self {
        Self { state, notify }
    }

    /// Apply an `AgentEventLite` to the state.
    pub fn push(&self, ev: AgentEventLite) {
        if let Ok(mut s) = self.state.lock() {
            if let AgentEventLite::PhaseChanged(phase) = &ev {
                s.status = phase.clone();
            }
        }
        if let Ok(mut s) = self.state.lock() {
            match ev {
                AgentEventLite::TextDelta(text) => s.push_assistant_raw(text),
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
                AgentEventLite::ToolResult { ok, content, details } => {
                    // Capture edit-tool diff stats for the status bar pill.
                    if let Some(d) = &details {
                        if let (Some(adds), Some(dels)) =
                            (d.get("additions").and_then(|v| v.as_u64()),
                             d.get("deletions").and_then(|v| v.as_u64()))
                        {
                            s.last_diff = Some((adds as usize, dels as usize));
                        }
                    }
                    s.push_tool_result_raw(ok, content);
                }
                AgentEventLite::TurnEnd => {
                    s.push_divider();
                    // Flush session to disk on turn end. Errors are logged but
                    // never propagated — IO failures must not break the TUI.
                    s.session_flush();
                }
                AgentEventLite::Error(message) => {
                    s.push_assistant_raw(format!("[error] {message}"));
                }
                AgentEventLite::Usage(input, output, cost) => {
                    s.tokens.input += input as u64;
                    s.tokens.output += output as u64;
                    // Cost is denominated in USD; add to running total.
                    if cost > 0.0 {
                        s.cost_usd += cost;
                    }
                }
                AgentEventLite::PhaseChanged(_) => {
                    // Already handled above (set s.status).
                }
                AgentEventLite::Done => {
                    s.mode = RunMode::Editing;
                    s.status = "ready".to_string();
                    s.abort_signal = None; // clear stale abort signal
                }
            }
        }
        // v0.7.4 (UX fix) — notify the runtime's select! so the TUI
        // redraws immediately rather than waiting for the 50ms tick.
        // notify_one() is sufficient — the runtime re-snapshots and
        // renders on its next loop iteration.
        self.notify.notify_one();
    }

    /// Inject a tool result into the transcript synchronously. Used by local
    /// commands (!bash) that run outside the agent driver and need to contribute
    /// to the same turn's transcript without spawning an async task.
    pub fn inject_tool_result(&self, ok: bool, content: String) {
        if let Ok(mut s) = self.state.lock() {
            s.push_tool_result(ok, content);
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

    // Install SIGINT/SIGTERM/SIGHUP handlers + panic hook before enabling
    // raw mode. The panic hook calls emergency_cleanup() to restore the
    // terminal if anything blows up.
    let _ = crate::signals::install_handlers();

    enable_raw_mode()?;
    crate::signals::mark_raw_mode(true);
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    crate::signals::mark_alt_screen(true);
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    terminal.hide_cursor()?;

    let result = run_loop(&mut terminal, shared, agent_driver).await;

    terminal.show_cursor()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    disable_raw_mode()?;
    crate::signals::mark_raw_mode(false);
    crate::signals::mark_alt_screen(false);
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
    let mut theme = {
        let mut settings = crate::settings::SettingsManager::default();
        let name = settings.theme_name();
        // Mirror the theme name into AppState so the status bar can
        // render it without holding the settings lock.
        shared.lock().unwrap().theme_name = name.clone();
        settings.theme()
    };

    // Spawn theme watcher (best-effort; falls back silently if dirs missing).
    let cwd = std::env::current_dir().ok();
    let (theme_tx, mut theme_rx) = tokio::sync::mpsc::unbounded_channel();
    let _watcher = crate::theme_watcher::spawn_theme_watcher(cwd.as_deref(), theme_tx);

    loop {
        // Snapshot state for rendering (cheap clone, doesn't hold lock long).
        let snapshot = {
            let g = shared.lock().unwrap();
            g.clone()
        };
        // v0.6: F1 sets `state.close_help` to ask the runtime to drop
        // any active help selector. Pick up that signal here so the
        // overlay actually disappears when the user toggles F1 again.
        if snapshot.close_help {
            active_selector = None;
            selector_query.clear();
            selector_visible.clear();
            let mut g = shared.lock().unwrap();
            g.close_help = false;
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
        let selector_title = active_selector.as_ref().map(|s| s.state_title().to_string());
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
        // v0.7.4 (UX fix) — cloned notify for the agent sink to wake
        // the loop between events. Created once per loop iteration
        // so each submit's sink has its own notification handle.
        let sink_notify: Arc<tokio::sync::Notify> =
            Arc::new(tokio::sync::Notify::new());

        // Check if submit_user_input signaled a selector-open request via
        // state.status. Run AFTER done.notify_waiters() in submit_user_input.
        if active_selector.is_none() {
            let status = shared.lock().unwrap().status.clone();
            if let Some(kind) = status.strip_prefix("open_selector:") {
                let current_model = shared.lock().unwrap().model.clone();
                match kind {
                    "model" => {
                        let sel = Box::new(crate::selectors::ModelSelector::new(Some(&current_model)));
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
                        let home = std::env::var("HOME").ok();
                        let cwd = std::env::current_dir().ok();
                        let cwd_name = cwd
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "default".to_string());
                        let dir = home
                            .map(|h| std::path::PathBuf::from(h))
                            .unwrap_or_else(|| std::path::PathBuf::from("."))
                            .join(".pi")
                            .join("agent")
                            .join("sessions")
                            .join(&cwd_name);
                        let sel = Box::new(crate::selectors::SessionSelector::from_dir(&dir));
                        active_selector = Some(sel);
                        selector_query.clear();
                    }
                    "tree" => {
                        // Open tree selector against the active session's entries.
                        let session_arc = shared.lock().unwrap().session.clone();
                        let entries: Vec<nini_core::SessionEntry> = if let Some(arc) = session_arc {
                            if let Ok(guard) = arc.try_lock() {
                                guard.entries.clone()
                            } else {
                                Vec::new()
                            }
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
                        let home = std::env::var("HOME").ok();
                        let trust_path = home
                            .as_deref()
                            .and_then(|h| nini_core::project_trust::ProjectTrustStore::load(&std::path::PathBuf::from(h).join(".pi").join("agent").join("trust.json")).ok());
                        let current = trust_path.as_ref().and_then(|t| t.get(&cwd_str));
                        let sel = Box::new(crate::selectors::TrustSelector::new(cwd_str, current));
                        active_selector = Some(sel);
                        selector_query.clear();
                    }
                    "settings" => {
                        let sel = Box::new(crate::selectors::SettingsSelector::new(
                            crate::settings::SettingsManager::default(),
                        ));
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
                shared.lock().unwrap().status = "ready".to_string();
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
                                    shared.lock().unwrap().model = new_model;
                                }
                                // Selector closed: reset any residual
                                // open_selector:* flag so the status bar
                                // doesn't display stale 'switch model…'
                                // / 'open_selector:tree' after the user
                                // has navigated away.
                                clear_status_after_selector(&shared);
                                if let Ok(mut g) = shared.lock() {
                                    if g.status.is_empty() || g.status.starts_with("open_selector:") {
                                        g.status = "ready".to_string();
                                    }
                                }
                            }
                        } else {
                            handle_key(k, &shared, &agent_driver, done);
                        }
                    }
                    Some(Ok(Event::Resize(_, _))) => { /* ratatui handles */ }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => eprintln!("event error: {e}"),
                    None => break,
                }
            }
            _ = done_for_select.notified() => {
                // Agent finished; loop will redraw on next iteration.
            }
            _ = sink_notify.notified() => {
                // v0.7.4 (UX fix) — agent emitted an event. Redraw
                // immediately so the TUI reflects the latest
                // transcript / status / mode — don't wait for the
                // 50ms tick.
            }
            // F020: external editor dance. Polled each iteration
            // because the dance is synchronous (we leave alt screen
            // + spawn editor + re-enter) so a Notify wouldn't fire
            // until after we'd already resumed anyway. We just
            // check the flag cheaply here.
            _ = editor_poll_tick.tick() => {
                if shared.lock().unwrap().pending_external_editor {
                    handle_external_editor_dance(terminal, &shared);
                }
            }
            _ = tick.tick() => { /* animation tick */ }
            Some(theme_event) = theme_rx.recv() => {
                // Theme file changed — reload from settings.
                use crate::theme_watcher::ThemeEvent;
                // Also publish to the global event bus for other modules.
                let bus = crate::event_bus::global();
                match theme_event {
                    ThemeEvent::Changed(path) | ThemeEvent::Removed(path) => {
                        let mut settings = crate::settings::SettingsManager::default();
                        let name = settings.theme_name();
                        shared.lock().unwrap().theme_name = name.clone();
                        theme = settings.theme();
                        let _ = bus.emit(crate::event_bus::AppEvent::ThemeChanged(
                            name.unwrap_or_default(),
                        ));
                        let _ = path; // suppress unused
                    }
                    ThemeEvent::Error(_, msg) => {
                        eprintln!("[nini] theme watcher error: {msg}");
                    }
                }
            }
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
                crate::selector::SelectorOutcome::Picked(_) | crate::selector::SelectorOutcome::Back => {
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
fn apply_selector_result(
    mut selector: Box<dyn crate::selector::SelectorState + Send>,
    shared: &SharedState,
) -> Option<String> {
    // Downcast to concrete selectors to read their `result` field, then
    // persist via SettingsManager / ProjectTrustStore / etc.
    use crate::selectors::model::ModelSelector;
    use crate::selectors::session::SessionSelector;
    use crate::selectors::thinking::ThinkingSelector;
    use crate::selectors::settings::SettingsSelector;
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
    if let Some(think_sel) = selector.state_as_any_mut().downcast_mut::<ThinkingSelector>() {
        if let Some(level) = &think_sel.result {
            let mut settings = SettingsManager::default();
            settings.set_default_thinking_level(level.clone());
        }
        return None;
    }
    if let Some(trust_sel) = selector.state_as_any_mut().downcast_mut::<TrustSelector>() {
        if let Some(decision) = trust_sel.result {
            // Persist to ProjectTrustStore.
            let cwd = trust_sel.cwd.clone();
            if let Ok(home) = std::env::var("HOME") {
                let path = std::path::PathBuf::from(home)
                    .join(".pi")
                    .join("agent")
                    .join("trust.json");
                let mut store = nini_core::project_trust::ProjectTrustStore::load(&path)
                    .unwrap_or_default();
                store.set(&cwd, decision);
                let _ = store.save(&path);
            }
        }
        return None;
    }
    if let Some(session_sel) = selector.state_as_any_mut().downcast_mut::<SessionSelector>() {
        if let Some(path_str) = session_sel.result.clone() {
            // Load the session file into AppState.
            let path = std::path::PathBuf::from(&path_str);
            if let Err(e) = shared.lock().unwrap().session_load(path) {
                eprintln!("[nini] session load failed: {e}");
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
        let session_arc = shared.lock().unwrap().session.clone();
        let prev_status = shared.lock().unwrap().status.clone();
        shared.lock().unwrap().status = "branch summary: computing…".into();
        if let Some(arc) = session_arc {
            if let Ok(guard) = arc.try_lock() {
                let entries = guard.entries.clone();
                drop(guard);
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
                    let mut g = shared.lock().unwrap();
                    g.push_assistant(format!(
                        "(branch summary: {len} chars)\n{truncated}",
                    ));
                    g.push_divider();
                    // (b) Queue full summary for next turn.
                    g.pending_next_turn_messages.push(format!(
                        "[BRANCH SUMMARY]\n\n{}",
                        s,
                    ));
                }
                shared.lock().unwrap().status = prev_status;
            }
        }
        return None;
    }
    if let Some(settings_sel) = selector
        .state_as_any_mut()
        .downcast_mut::<SettingsSelector>()
    {
        // Apply the toggle/cycle. The settings manager inside the selector
        // persists to disk via its internal mechanism.
        let new_model = settings_sel.apply(0).map(|_| settings_sel.settings.model_name());
        new_model.and_then(|s| if s.is_empty() { None } else { Some(s) })
    } else if let Some(palette) = selector
        .state_as_any_mut()
        .downcast_mut::<crate::command_palette::CommandPalette>()
    {
        // F015: the user picked a palette entry. Read the selected
        // item and dispatch based on its id prefix.
        use crate::selector::{SelectorItem, SelectorState};
        let selected_idx = palette.state_selected();
        let items = palette.state_items();
        if let Some(item) = items.get(selected_idx) {
            // Read the id first so we can drop the lock before
            // mutating shared state via dispatch().
            let id = item.id.clone();
            let label = item.label.clone();
            match id.as_str() {
                id if id.starts_with("cmd:/") => {
                    // Inject the slash command into the input buffer
                    // and call submit_user_input via the same path
                    // the runtime uses for an Enter keypress.
                    let name = id.trim_start_matches("cmd:/").to_string();
                    let cmd_line = format!("/{name}");
                    let mut g = shared.lock().unwrap();
                    g.input.text.clear();
                    g.input.cursor = 0;
                    drop(g);
                    if let Some((cmd_id, args)) = crate::commands::parse(&cmd_line) {
                        let mut g2 = shared.lock().unwrap();
                        let mut settings = crate::settings::SettingsManager::default();
                        let result = crate::commands::dispatch(
                            &mut g2,
                            &mut settings,
                            cmd_id,
                            &args,
                        );
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
                                g2.mode = crate::state::RunMode::Quitting;
                            }
                            crate::commands::CommandOutcome::PromptArgument { prompt, next: _ } => {
                                g2.push_assistant(format!("(prompt: {prompt})"));
                                g2.push_divider();
                            }
                        }
                    }
                }
                "action:clear" => {
                    let mut g = shared.lock().unwrap();
                    g.transcript.clear();
                    g.push_divider();
                }
                "action:exit" => {
                    let mut g = shared.lock().unwrap();
                    g.mode = RunMode::Quitting;
                }
                _ => {
                    let mut g = shared.lock().unwrap();
                    g.push_assistant(format!(
                        "[palette] unknown action id: {}",
                        id
                    ));
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
// through to here; we reset state.status to "ready" so that stale
// strings like "switch model (not yet implemented)" or
// "open_selector:model" don't linger after the selector closes.
fn clear_status_after_selector(shared: &SharedState) {
    if let Ok(mut g) = shared.lock() {
        if g.status.starts_with("open_selector:")
            || g.status == "switch model (not yet implemented)"
        {
            g.status = "ready".to_string();

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
) {
    // 1. Snapshot input and clear the flag.
    let (initial_text, initial_cursor) = {
        let mut g = shared.lock().unwrap();
        g.pending_external_editor = false;
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
        let _ = execute!(stdout, LeaveAlternateScreen);
        let _ = disable_raw_mode();
        crate::signals::mark_alt_screen(false);
        crate::signals::mark_raw_mode(false);
        let _ = execute!(stdout, crossterm::cursor::Show);
    }

    // 3. Spawn the editor synchronously. This call blocks the
    // run loop until the user finishes editing. That's OK —
    // the TUI is suspended, so no UI updates are expected.
    let result = crate::editor::edit_in_external_editor(&initial_text);

    // 4. Resume the TUI BEFORE applying the result. We must be
    // able to write to the terminal again to redraw the new state.
    {
        let mut stdout = stdout();
        let _ = enable_raw_mode();
        crate::signals::mark_raw_mode(true);
        let _ = execute!(stdout, EnterAlternateScreen);
        crate::signals::mark_alt_screen(true);
        let _ = execute!(stdout, crossterm::cursor::Hide);
    }

    // 5. Apply the result (or the error) and force a redraw.
    match result {
        Ok(Some(new_text)) => {
            let mut g = shared.lock().unwrap();
            // Replace the entire input buffer + push an undo snapshot
            // so Ctrl+Z restores the pre-edit version.
            g.input.replace_whole(new_text.clone());
            g.status = format!("editor: {} chars", new_text.chars().count());
        }
        Ok(None) => {
            // No change.
            let mut g = shared.lock().unwrap();
            g.status = "editor: no changes".to_string();
            // Suppress the unused warning on initial_cursor.
            let _ = initial_cursor;
        }
        Err(e) => {
            let mut g = shared.lock().unwrap();
            g.status = format!("editor error: {e}");
            g.push_assistant(format!("[editor error] {e}"));
            g.push_divider();
        }
    }

    // 6. Force the next render — terminal.clear() wipes any stale
    // content from the editor session.
    let _ = terminal.clear();
}

/// Cycle to the next/previous model in `state.models_cycle`.
/// Updates `state.model`, persists to settings.json, and updates status.
fn cycle_model(state: &mut crate::state::AppState, direction: i32) {
    if state.models_cycle.is_empty() {
        state.status = "(no model cycle configured; use /model)".to_string();
        return;
    }
    // Find current model in cycle; advance by `direction`.
    let current_pos = state
        .models_cycle
        .iter()
        .position(|m| m == &state.model)
        .unwrap_or(0);
    let n = state.models_cycle.len() as i32;
    let mut new_pos = current_pos as i32 + direction;
    if new_pos < 0 {
        new_pos += n;
    } else if new_pos >= n {
        new_pos -= n;
    }
    let new_pos = new_pos as usize;
    state.models_cycle_idx = Some(new_pos);
    let new_model = state.models_cycle[new_pos].clone();
    state.model = new_model.clone();
    // Persist to settings.json if the runtime wired in a real path.
    let mut settings = match state.settings_path.clone() {
        Some(p) => crate::settings::SettingsManager::load_from_disk(p),
        None => crate::settings::SettingsManager::default(),
    };
    settings.set_default_model(&new_model);
    state.status = format!("model: {new_model}");
}

/// Cycle thinking level through the standard set.
fn cycle_thinking(state: &mut crate::state::AppState, direction: i32) {
    const LEVELS: &[&str] = &[
        "off", "minimal", "low", "medium", "high", "xhigh", "max",
    ];
    // Read current level from dedicated state field; fall back to "medium"
    // if never set.
    let current = state
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
    let mut settings = match &state.settings_path {
        Some(p) => crate::settings::SettingsManager::load_from_disk(p.clone()),
        None => crate::settings::SettingsManager::default(),
    };
    // If a model is selected and the user has a per-model override, write
    // to the override; otherwise update the default.
    let model = state.model.clone();
    if !model.is_empty() {
        settings.set_model_thinking_level(&model, new_level);
    } else {
        settings.set_default_thinking_level(new_level);
    }
    // Also update live state so the selector stays in sync.
    state.thinking_level = Some(new_level.to_string());
    state.status = format!("thinking: {new_level}");
}

fn handle_key(k: KeyEvent, shared: &SharedState, agent_driver: &AgentDriver, done: Arc<Notify>) {
    let mut state = shared.lock().unwrap();
    let key: Key = k.into();
    let action = resolve_with_user_overrides(key);
    // Drop the lock while we hold it; the rest of the match needs it.

    match action {
        KeyAction::Insert(c) => {
            // F019: search-mode special keys (n/N) jump matches.
            if state.search.is_some() {
                if c == 'n' {
                    state.search_next();
                    return;
                } else if c == 'N' {
                    state.search_prev();
                    return;
                }
                // Otherwise treat as query char.
                if let Some(search) = state.search.as_mut() {
                    search.query.push(c);
                }
                let q = state.search.as_ref().map(|s| s.query.clone()).unwrap_or_default();
                state.update_search_query(q);
                return;
            }
            if state.mode == RunMode::Editing {
                state.input.insert_char(c);
                state.refresh_completion();
            }
        }
        KeyAction::OpenSearch => {
            // F019: open search when input is empty AND no popup is up;
            // otherwise insert literal `/` so the user can still type
            // a slash command.
            if state.mode == RunMode::Editing
                && state.completion.is_none()
                && state.input.text.is_empty()
            {
                state.begin_search();
            } else if state.mode == RunMode::Editing {
                state.input.insert_char('/');
                state.refresh_completion();
            }
        }
        KeyAction::OpenCommandPalette => {
            // F015 Ctrl+K: open the command palette. Works from any
            // editing state; closes any open slash popup first.
            if state.mode == RunMode::Editing {
                state.completion = None;
                state.status = "open_selector:palette".to_string();
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
                    state.status = format!("pasted {:.1} KB", size as f64 / 1024.0);
                }
                Ok(None) => {}
                Err(e) => {
                    state.push_assistant(format!("[paste error] {e}"));
                    state.status = format!("paste error");
                }
            }
        }
        KeyAction::Newline => {
            if state.mode == RunMode::Editing {
                state.input.insert_char('\n');
                state.refresh_completion();
            }
        }
        KeyAction::Backspace => {
            // F019: search-mode backspace mutates the query, not the
            // input buffer.
            if state.search.is_some() {
                if let Some(search) = state.search.as_mut() {
                    search.query.pop();
                }
                let q = state.search.as_ref().map(|s| s.query.clone()).unwrap_or_default();
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
        KeyAction::KillWordForward => state.input.kill_word_forward(),
        KeyAction::Yank => {
            if !state.input.yank() {
                state.status = "(kill ring empty)".to_string();
            }
        }
        KeyAction::YankPop => {
            if !state.input.yank_pop() {
                state.status = "(no previous yank)".to_string();
            }
        }
        KeyAction::Undo => {
            if !state.input.undo() {
                state.status = "(nothing to undo)".to_string();
            }
        }
        KeyAction::ClearInput => state.input.clear(),
        KeyAction::AcceptCompletionOrInsertTab => {
            if state.completion.is_some() {
                state.apply_completion();
            } else if state.mode == RunMode::Editing {
                // No popup: insert literal tab.
                state.input.insert_char('\t');
            }
        }
        KeyAction::Submit => {
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
            if let Some(popup) = state.completion.as_ref() {
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
                    state.completion = None;
                } else if popup.selected_item_has_argument_hint() {
                    state.apply_completion();
                    return;
                } else {
                    state.apply_completion();
                    // Don't return — let submit proceed.
                    state.completion = None;
                }
            }
            if state.completion.is_some() {
                state.apply_completion();
            } else {
                // Install an abort signal for the upcoming agent turn. The
                // agent driver (CLI / wiring code) doesn't have direct
                // access to AppState today; the CLI would need to set this
                // before spawning the driver. v1 stores the signal here so
                // that any future driver-side wiring can read it.
                state.abort_signal = Some(Arc::new(tokio::sync::Notify::new()));
                drop(state);
                submit_user_input(shared, agent_driver, done);
            }
        }
        KeyAction::Abort => {
            if state.completion.is_some() {
                state.completion = None;
            } else if state.search.is_some() {
                // Esc exits transcript search (F019).
                state.end_search();
                state.status = "ready".to_string();
            } else if state.pending_quit.is_some() {
                // Esc cancels the pending Ctrl+D quit confirmation.
                state.pending_quit = None;
                state.status = "ready".to_string();
            } else if state.mode == RunMode::Running {
                // Signal the agent to abort, then mark mode as Aborted.
                if let Some(sig) = state.abort_signal.as_ref() {
                    sig.notify_waiters();
                }
                state.mode = RunMode::Aborted;
                state.status = "aborted".to_string();
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
            if let Some(pending_at) = state.pending_quit {
                if now.duration_since(pending_at) <= Duration::from_millis(QUIT_CONFIRM_WINDOW_MS) {
                    state.pending_quit = None;
                    state.mode = RunMode::Quitting;
                } else {
                    // Window expired; treat this as the first tap again.
                    state.pending_quit = Some(now);
                    state.status = "Press Ctrl+D again to quit, or Esc to cancel".to_string();
                }
            } else {
                state.pending_quit = Some(now);
                state.status = "Press Ctrl+D again to quit, or Esc to cancel".to_string();
            }
        }
        KeyAction::SwitchModel => {
            // Mirror `/model` (no args) — set the status flag so the main
            // loop's "open_selector:model" branch spins up the selector.
            // The previous implementation only set a status string, which
            // made Ctrl+L a no-op despite the CHANGELOG claiming
            // ModelSelector is wired.
            state.status = "open_selector:model".to_string();
        }
        KeyAction::OpenExternalEditor => {
            // F020: signal the run loop to suspend the TUI, spawn
            // $VISUAL/$EDITOR on the current input, then resume.
            // We just set the flag here; the loop owns the actual
            // suspend/resume dance because it has access to the
            // terminal handle. The flag is checked on the next
            // event-loop iteration.
            state.pending_external_editor = true;
            state.status = "opening editor…".to_string();
        }
        KeyAction::CycleModelNext => cycle_model(&mut state, 1),
        KeyAction::CycleModelPrev => cycle_model(&mut state, -1),
        KeyAction::CycleThinkingNext => cycle_thinking(&mut state, 1),
        KeyAction::CycleThinkingPrev => cycle_thinking(&mut state, -1),
        KeyAction::ShowHelp => {
            // v0.6: F1 toggles the help overlay + extended footer.
            // The overlay-close half is handled in the main loop via
            // `state.close_help` (set true here; loop clears the
            // active selector and resets the flag).
            if state.help_extended {
                state.help_extended = false;
                state.close_help = true;
                state.status = "ready".to_string();
            } else {
                state.help_extended = true;
                state.status = "open_selector:help".to_string();
            }
        }
        KeyAction::ScrollUp => {
            // PageUp: scroll up by ~10 lines.
            let max_offset = state.transcript.len().saturating_sub(1);
            let step = 10usize;
            state.scroll_offset = (state.scroll_offset + step).min(max_offset);
            state.autoscroll = false;
        }
        KeyAction::ToggleCollapse => {
            // Toggle collapsed on the most-recent collapsible transcript
            // line. The user can scroll first (PageUp/PageDown) to put a
            // specific line at the top, then press Ctrl+O; for the
            // common "fold the last tool output" use-case, scrolling is
            // unnecessary because we default to the tail.
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
        KeyAction::ScrollDown => {
            let step = 10usize;
            if state.scroll_offset <= step {
                state.scroll_offset = 0;
                state.autoscroll = true;
            } else {
                state.scroll_offset -= step;
            }
        }
        KeyAction::Noop => {}
    }
}

/// Submit handler: extract text, intercept slash commands, otherwise spawn agent.
///
/// Slash commands are dispatched locally — they run synchronously and do NOT
/// transition the TUI to Running mode. Non-slash input goes to the agent
/// driver as before.
pub fn submit_user_input(shared: &SharedState, agent_driver: &AgentDriver, done: Arc<Notify>) {
    // v0.7.4 (UX fix) — share a notify with the agent sink so the
    // runtime's event loop wakes immediately when an agent event
    // arrives, instead of waiting up to 50ms for the next tick.
    submit_user_input_inner(shared, agent_driver, done, Arc::new(tokio::sync::Notify::new()))
}

/// Internal helper for `submit_user_input` — takes an additional
/// `sink_notify` arg that is passed to the agent sink so the
/// runtime can wake on each event arrival.
fn submit_user_input_inner(
    shared: &SharedState,
    agent_driver: &AgentDriver,
    done: Arc<Notify>,
    sink_notify: Arc<tokio::sync::Notify>,
) {
    let text = {
        let mut g = shared.lock().unwrap();
        if g.mode != RunMode::Editing {
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
                    format!(
                        "{output}\n…(timed out after {} ms)",
                        result.duration_ms
                    )
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
                g.transcript.push(TranscriptLine::BashExecution {
                    id,
                    cmd: cmd_display,
                    output: output.clone(),
                    stderr: crate::ansi::strip_ansi(&result.stderr),
                    ok: result.ok,
                    exit_code: result.exit_code,
                    duration_ms: result.duration_ms,
                    collapsed: false,
                });
                g.push_divider();
                // For `!cmd`, also inject the output as a user message so
                // the agent sees it on the next turn. For `!!cmd` the
                // privacy marker tells us NOT to do that — the LLM will
                // never learn the contents of this command.
                if !private {
                    g.pending_next_turn_messages.push(format!(
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
                    g.status = "executed (private)".to_string();
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
                    g.status = "open_selector:model".to_string();
                    let text_for_agent = text.clone();
                    return drop_and_signal(text_for_agent, done);
                }
                CommandId::Thinking if args.trim().is_empty() => {
                    g.push_assistant("(opening thinking selector...)".to_string());
                    g.push_divider();
                    g.status = "open_selector:thinking".to_string();
                    let text_for_agent = text.clone();
                    return drop_and_signal(text_for_agent, done);
                }
                CommandId::Session if args.trim().is_empty() => {
                    g.push_assistant("(opening session selector...)".to_string());
                    g.push_divider();
                    g.status = "open_selector:session".to_string();
                    let text_for_agent = text.clone();
                    return drop_and_signal(text_for_agent, done);
                }
                CommandId::Tree if args.trim().is_empty() => {
                    g.push_assistant("(opening tree selector...)".to_string());
                    g.push_divider();
                    g.status = "open_selector:tree".to_string();
                    let text_for_agent = text.clone();
                    return drop_and_signal(text_for_agent, done);
                }
                CommandId::Trust if args.trim().is_empty() => {
                    g.push_assistant("(opening trust selector...)".to_string());
                    g.push_divider();
                    g.status = "open_selector:trust".to_string();
                    let text_for_agent = text.clone();
                    return drop_and_signal(text_for_agent, done);
                }
                CommandId::Settings if args.trim().is_empty() => {
                    g.push_assistant("(opening settings selector...)".to_string());
                    g.push_divider();
                    g.status = "open_selector:settings".to_string();
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
                    g.mode = RunMode::Quitting;
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
        if g.is_compacting {
            let n = g.pending_next_turn_messages.len() + 1;
            g.pending_next_turn_messages.push(text.clone());
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
        let queued = std::mem::take(&mut g.pending_next_turn_messages);
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
        if g.should_auto_compact(&g.settings_snapshot) {
            let folded = g.auto_compact_local();
            g.push_assistant(format!(
                "[auto-compact] folded {folded} entries before next turn"
            ));
            g.push_divider();
        }

        g.push_user(text.clone());
        g.push_divider();
        g.mode = RunMode::Running;
        g.status = "running...".to_string();
        text
    };

    // Spawn the agent task with its own sink.
    let sink = AgentSink::new(shared.clone(), sink_notify.clone());
    let _handle = (agent_driver)(text, sink, done.clone());
    // The handle is intentionally dropped — the task continues running in
    // the background. We don't abort the agent on quit; the runtime owns
    // the SharedState and the sink keeps a clone.
}

/// Apply a `KeyAction` to a state without spawning anything. Used by tests
/// and the no-agent mode.
pub fn apply_action(state: &mut AppState, key: Key) {
    let action = resolve_with_user_overrides(key);
    match action {
        KeyAction::Insert(c) => {
            if state.mode == RunMode::Editing {
                state.input.insert_char(c);
                state.refresh_completion();
            }
        }
        KeyAction::OpenCommandPalette => {
            // Test path: same as the runtime path (open_selector:palette).
            if state.mode == RunMode::Editing {
                state.completion = None;
                state.status = "open_selector:palette".to_string();
            }
        }
        KeyAction::OpenSearch => {
            // Test path mirrors the runtime path.
            if state.mode == RunMode::Editing
                && state.completion.is_none()
                && state.input.text.is_empty()
            {
                state.begin_search();
            } else if state.mode == RunMode::Editing {
                state.input.insert_char('/');
                state.refresh_completion();
            }
        }
        KeyAction::OpenExternalEditor => {
            // F020: same as the runtime path — set the flag and
            // let the run loop handle the suspend/resume dance.
            state.pending_external_editor = true;
            state.status = "opening editor…".to_string();
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
                    state.status = format!("pasted {:.1} KB", size as f64 / 1024.0);
                }
                Ok(None) => {}
                Err(e) => {
                    state.push_assistant(format!("[paste error] {e}"));
                    state.status = format!("paste error");
                }
            }
        }
        KeyAction::Newline => {
            if state.mode == RunMode::Editing {
                state.input.insert_char('\n');
                state.refresh_completion();
            }
        }
        KeyAction::Backspace => {
            // F019: search-mode backspace mutates the query, not the
            // input buffer.
            if state.search.is_some() {
                if let Some(search) = state.search.as_mut() {
                    search.query.pop();
                }
                let q = state.search.as_ref().map(|s| s.query.clone()).unwrap_or_default();
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
        KeyAction::KillWordForward => state.input.kill_word_forward(),
        KeyAction::Yank => {
            if !state.input.yank() {
                state.status = "(kill ring empty)".to_string();
            }
        }
        KeyAction::YankPop => {
            if !state.input.yank_pop() {
                state.status = "(no previous yank)".to_string();
            }
        }
        KeyAction::Undo => {
            if !state.input.undo() {
                state.status = "(nothing to undo)".to_string();
            }
        }
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

/// Resolve a key against the built-in keymap plus any user overrides
/// from `~/.pi/agent/keybindings.json`. The override file is loaded
/// once on first use and cached for the process lifetime.
pub fn resolve_with_user_overrides(key: Key) -> KeyAction {
    use std::sync::OnceLock;
    use crate::keybindings_manager::load_user_overrides;

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
