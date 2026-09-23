//! App state for the TUI.
//!
//! The state is a small, observable model: prompt input + cursor, conversation
//! log (transcript lines), run mode (editing vs. running vs. aborted),
//! and minimal model/session metadata. Render is a pure function of state.
//!
//! All editor mutations are pure functions on `InputBuffer`, which makes
//! them trivially testable without a terminal.

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use unicode_segmentation::UnicodeSegmentation;

/// One row of the conversation transcript. Either a user message, assistant
/// text, a tool call (with tool name + JSON args), or a tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptLine {
    User(String),
    AssistantText(String),
    /// Tool invocation. `args` is rendered as a one-line JSON preview.
    ToolCall {
        name: String,
        args: String,
        /// When true the rendered preview is collapsed to a single line and
        /// the full args are hidden until the user toggles via Ctrl+O.
        collapsed: bool,
    },
    /// Tool result. `Ok`/`Err` reflects `ToolOutput.is_error`.
    ToolResult {
        ok: bool,
        content: String,
        /// When true the body of the result is hidden — only a one-line
        /// summary is shown. Ctrl+O toggles.
        collapsed: bool,
    },
    /// System-injected divider (turn boundary).
    Divider,
    /// A bash execution rendered as a self-contained component.
    /// Used by `!cmd` passthrough and (future) bash tool calls.
    BashExecution {
        id: String,
        cmd: String,
        output: String,
        /// Standard error captured separately so the UI can render it in
        /// `error` color. Empty when the command produced no stderr.
        stderr: String,
        ok: bool,
        exit_code: Option<i32>,
        duration_ms: u64,
        /// When true the multi-line output is hidden behind a header
        /// summary. Ctrl+O toggles.
        collapsed: bool,
    },
}

impl TranscriptLine {
    /// If this line is `AssistantText`, return a reference to the text.
    pub fn as_assistant_text(&self) -> Option<&str> {
        match self {
            TranscriptLine::AssistantText(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Compact one-line summary suitable for transcript search (F019)
    /// and search UI. Includes a `[type]` prefix so users searching
    /// for things like 'tool-call' or 'user' can find them without
    /// needing to know the underlying structure.
    pub fn summary_text(&self) -> String {
        line_summary_text_impl(self)
    }
}

/// Editable prompt buffer with cursor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputBuffer {
    /// Cursor position as a **byte offset** into `text`. We keep it as bytes
    /// and adjust via `text.len()` clamping for simplicity (correct for
    /// ASCII; conservative but valid for multi-byte UTF-8 too since we only
    /// insert whole characters at this offset).
    pub text: String,
    /// Cursor byte offset (0..=text.len()).
    pub cursor: usize,
    /// History of previously submitted messages (most recent at end).
    pub history: Vec<String>,
    /// Cursor into `history` for Up/Down navigation. `None` = editing live.
    pub history_cursor: Option<usize>,
    /// Kill ring: most recent killed text at the end. New kills push; yanks
    /// pop from the end; yank-pop rotates backward.
    pub kill_ring: Vec<String>,
    /// Cursor position within `kill_ring` for yank-pop. `None` = no
    /// pending yank.
    pub kill_ring_cursor: Option<usize>,
    /// Undo stack: snapshots of (text, cursor) before each user edit.
    /// Ctrl+/ undoes the most recent.
    pub undo_stack: Vec<(String, usize)>,
}

impl InputBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// True if the current input starts with `/` and is on its first token
    /// (i.e., a slash-command candidate). Used by the autocomplete layer.
    pub fn is_slash_command(&self) -> bool {
        let t = self.text.trim_start();
        t.starts_with('/') && !t.contains('\n')
    }

    /// If the input starts with `/<partial>`, return the partial command
    /// name (without the leading `/`). Otherwise return None.
    pub fn slash_prefix(&self) -> Option<&str> {
        let t = self.text.trim_start();
        if !t.starts_with('/') {
            return None;
        }
        let rest = &t[1..];
        // Only return partial if there's no whitespace yet (still typing cmd name).
        if rest.is_empty() || rest.split_whitespace().next().unwrap_or(rest) == rest {
            Some(rest)
        } else {
            None
        }
    }

    pub fn insert_char(&mut self, c: char) {
        self.push_undo_snapshot();
        // Insert at cursor (byte position). Caller must ensure char boundary.
        let bytes = c.len_utf8();
        let pos = self.cursor.min(self.text.len());
        self.text.insert(pos, c);
        self.cursor = pos + bytes;
    }

    pub fn insert_str(&mut self, s: &str) {
        self.push_undo_snapshot();
        let pos = self.cursor.min(self.text.len());
        self.text.insert_str(pos, s);
        self.cursor += s.len();
    }

    /// Backspace: delete the char before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.push_undo_snapshot();
        // Find previous char boundary
        let prev = prev_char_boundary(&self.text, self.cursor);
        self.text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
    }

    /// Delete: delete the char at the cursor.
    pub fn delete(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        self.push_undo_snapshot();
        let next = next_char_boundary(&self.text, self.cursor);
        self.text.replace_range(self.cursor..next, "");
    }

    pub fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor = prev_char_boundary(&self.text, self.cursor);
    }

    pub fn move_right(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        self.cursor = next_char_boundary(&self.text, self.cursor);
    }

    pub fn move_to_start(&mut self) {
        self.cursor = 0;
    }

    pub fn move_to_end(&mut self) {
        self.cursor = self.text.len();
    }

    /// Move left by one word (whitespace-bounded). Lands at the position
    /// just before the trailing non-whitespace run, mirroring `readline` /
    /// `bash` Alt+Left behavior. Operates on whole characters so multi-byte
    /// UTF-8 (e.g. CJK) is handled correctly.
    pub fn move_word_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        // Convert the prefix to chars and work with char indices, then map
        // back to byte positions. This is O(n) per call but `n` is the line
        // length, which is bounded by terminal width.
        let prefix: Vec<char> = self.text[..self.cursor].chars().collect();
        let mut i = prefix.len();
        // Phase 1: skip whitespace before cursor.
        while i > 0 && prefix[i - 1].is_whitespace() {
            i -= 1;
        }
        // Phase 2: skip non-whitespace.
        while i > 0 && !prefix[i - 1].is_whitespace() {
            i -= 1;
        }
        self.cursor = prefix[..i].iter().map(|c| c.len_utf8()).sum();
    }

    /// Move right by one word.
    pub fn move_word_right(&mut self) {
        let s = &self.text[self.cursor..];
        let mut new_pos = self.cursor;
        // Skip current word's remaining chars
        let mut iter = s.graphemes(true).peekable();
        let mut consumed = 0;
        let mut in_word = false;
        for g in iter.by_ref() {
            let is_ws = g.chars().all(|c| c.is_whitespace());
            if !is_ws {
                in_word = true;
                consumed += g.len();
            } else if in_word {
                break;
            }
        }
        new_pos += consumed;
        // Skip trailing whitespace
        while let Some(g) = iter.peek() {
            if g.chars().all(|c| c.is_whitespace()) {
                new_pos += g.len();
                iter.next();
            } else {
                break;
            }
        }
        self.cursor = new_pos.min(self.text.len());
    }

    pub fn kill_to_line_start(&mut self) {
        let prev = prev_word_boundary(&self.text, self.cursor);
        let killed = self.text[prev..self.cursor].to_string();
        if !killed.is_empty() {
            self.push_kill(killed);
        }
        self.text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
        self.push_undo_snapshot();
    }

    pub fn kill_to_line_end(&mut self) {
        let killed = self.text[self.cursor..].to_string();
        if !killed.is_empty() {
            self.push_kill(killed);
            self.push_undo_snapshot();
        }
        self.text.truncate(self.cursor);
    }

    pub fn kill_word_backward(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let s = &self.text[..self.cursor];
        let trimmed = s.trim_end_matches(|c: char| !c.is_whitespace());
        let prev = trimmed.trim_end_matches(|c: char| c.is_whitespace()).len();
        let killed = self.text[prev..self.cursor].to_string();
        if !killed.is_empty() {
            self.push_kill(killed);
            self.push_undo_snapshot();
        }
        self.text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
    }

    /// Kill forward (Alt+d) — same as kill_word_backward but going right.
    pub fn kill_word_forward(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        let s = &self.text[self.cursor..];
        // Find end of next word.
        let mut end = self.cursor;
        let mut chars = s.char_indices();
        // Skip non-word chars.
        while let Some((_, c)) = chars.clone().next() {
            if c.is_alphanumeric() || c == '_' {
                break;
            }
            end += c.len_utf8();
            chars.next();
        }
        // Consume word.
        while let Some((_, c)) = chars.clone().next() {
            if !(c.is_alphanumeric() || c == '_') {
                break;
            }
            end += c.len_utf8();
            chars.next();
        }
        let killed = self.text[self.cursor..end].to_string();
        if !killed.is_empty() {
            self.push_kill(killed);
        }
        self.text.replace_range(self.cursor..end, "");
        self.push_undo_snapshot();
    }

    /// Push killed text onto the kill ring. Consecutive kills (without a
    /// yank between them) append to the most recent slot.
    fn push_kill(&mut self, killed: String) {
        if let Some(last) = self.kill_ring.last_mut() {
            // bash-style: consecutive kills append.
            last.push_str(&killed);
            return;
        }
        self.kill_ring.push(killed);
    }

    /// Yank the most recent kill at the cursor.
    /// Returns true if a yank happened.
    pub fn yank(&mut self) -> bool {
        if self.kill_ring.is_empty() {
            return false;
        }
        let text = self.kill_ring.last().cloned().unwrap_or_default();
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
        // Yank-pop starts at the last item.
        self.kill_ring_cursor = Some(self.kill_ring.len() - 1);
        self.push_undo_snapshot();
        true
    }

    /// Yank-pop: replace the last yank with the previous kill ring entry.
    /// Wraps around.
    pub fn yank_pop(&mut self) -> bool {
        if self.kill_ring.len() < 2 {
            return false;
        }
        let cur = self.kill_ring_cursor.unwrap_or(self.kill_ring.len() - 1);
        // Remove the last yank from the buffer first.
        if let Some(last) = self.kill_ring.last() {
            let len = last.len();
            if self.cursor >= len {
                // Back up by last-yank length to restore the prior state.
                // (Approximation: works for the common case.)
                self.text.replace_range((self.cursor - len)..self.cursor, "");
                self.cursor -= len;
            }
        }
        let prev = if cur == 0 { self.kill_ring.len() - 1 } else { cur - 1 };
        let text = self.kill_ring[prev].clone();
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.kill_ring_cursor = Some(prev);
        self.push_undo_snapshot();
        true
    }

    /// Push a snapshot for undo. Called automatically by edit operations.
    fn push_undo_snapshot(&mut self) {
        const MAX_UNDO: usize = 64;
        // Skip if the most recent snapshot is identical (no-op edit).
        if let Some(last) = self.undo_stack.last() {
            if last.0 == self.text && last.1 == self.cursor {
                return;
            }
        }
        self.undo_stack.push((self.text.clone(), self.cursor));
        if self.undo_stack.len() > MAX_UNDO {
            self.undo_stack.remove(0);
        }
    }

    /// Undo: pop the most recent snapshot. Returns true if anything happened.
    pub fn undo(&mut self) -> bool {
        if let Some((text, cursor)) = self.undo_stack.pop() {
            self.text = text;
            self.cursor = cursor;
            true
        } else {
            false
        }
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.history_cursor = None;
    }

    /// Replace the entire input buffer with `new_text`, snapshotting
    /// the pre-replace state for Ctrl+Z to restore. The cursor lands
    /// at the end of the new text (clamped to its byte length).
    ///
    /// F020 — used by the external editor dance when the user comes
    /// back from `$VISUAL` / `$EDITOR` with modified contents.
    pub fn replace_whole(&mut self, new_text: String) {
        if new_text == self.text {
            return; // no-op
        }
        self.push_undo_snapshot();
        self.text = new_text;
        self.cursor = self.text.len();
    }

    /// Submit the current input. Returns the submitted text.
    pub fn submit(&mut self) -> String {
        let text = self.text.clone();
        if !text.trim().is_empty() {
            self.history.push(text.clone());
        }
        self.text.clear();
        self.cursor = 0;
        self.history_cursor = None;
        text
    }

    /// Recall previous history entry. `direction` < 0 = older, > 0 = newer.
    pub fn recall_history(&mut self, direction: i32) {
        if self.history.is_empty() {
            return;
        }
        let cur = self.history_cursor.unwrap_or(self.history.len());
        let next = if direction < 0 {
            cur.saturating_sub(1)
        } else {
            cur.saturating_add(1).min(self.history.len())
        };
        if next >= self.history.len() {
            self.text.clear();
            self.cursor = 0;
            self.history_cursor = None;
        } else {
            self.text = self.history[next].clone();
            self.cursor = self.text.len();
            self.history_cursor = Some(next);
        }
    }
}

/// Find the previous char boundary (start of the char ending at `pos`).
/// For example, in "hé" (3 bytes), pos=2 (between é and the rest) returns 1.
fn prev_char_boundary(s: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let bytes = s.as_bytes();
    if pos > bytes.len() {
        return bytes.len();
    }
    // Step back one byte, then walk back through any continuation bytes.
    let mut i = pos - 1;
    while i > 0 && (bytes[i] & 0b1100_0000) == 0b1000_0000 {
        i -= 1;
    }
    i
}

/// Find the next char boundary strictly after `pos`.
fn next_char_boundary(s: &str, pos: usize) -> usize {
    let bytes = s.as_bytes();
    if pos >= bytes.len() {
        return bytes.len();
    }
    let mut i = pos + 1;
    while i < bytes.len() && (bytes[i] & 0b1100_0000) == 0b1000_0000 {
        i += 1;
    }
    i
}

fn prev_word_boundary(s: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let prefix = &s[..pos];
    let trimmed = prefix.trim_end_matches(|c: char| !c.is_whitespace());
    trimmed.trim_end_matches(|c: char| c.is_whitespace()).len()
}

/// What the TUI is currently doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// User is editing the prompt; nothing is running.
    Editing,
    /// Agent is executing a turn.
    Running,
    /// User aborted the current turn (Esc / Ctrl+C).
    Aborted,
    /// TUI is shutting down.
    Quitting,
}

/// Aggregated token usage for the status bar.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TokenStats {
    pub input: u64,
    pub output: u64,
}

/// A popup overlaid on the prompt: shows slash-command completions (or
/// file completions in future work). `None` means no popup visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionPopup {
    /// The items to display. Each entry has the command name + description.
    pub items: Vec<CompletionItem>,
    /// Currently selected item index (highlighted). 0-based, into `items`.
    pub selected: usize,
    /// Scroll offset into `items` for the popup's viewport. Lets the
    /// popup show results beyond the visible window without losing the
    /// user's selection position. 0-based, inclusive.
    pub scroll_offset: usize,
    /// How many rows the renderer can show. Default 8 to match v0.5's
    /// hard cap; can be tuned by callers.
    pub max_visible: usize,
}

/// One entry in the completion popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub name: String,
    pub description: String,
    /// Optional argument hint shown in dim text after the name.
    pub argument_hint: Option<String>,
}

/// Default viewport size for the scrollable completion popup. v0.5
/// hard-capped at 8, which made 15 of 23 commands invisible; v0.6 keeps
/// the same default so the UX doesn't shift, but callers can shrink /
/// grow it.
pub const DEFAULT_COMPLETION_VISIBLE: usize = 8;

impl CompletionPopup {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            max_visible: DEFAULT_COMPLETION_VISIBLE,
        }
    }

    pub fn with_items(items: Vec<CompletionItem>) -> Self {
        let mut s = Self::new();
        s.items = items;
        s
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Total number of items in the popup.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when the popup has more items than fit in the viewport.
    pub fn needs_scroll(&self) -> bool {
        self.items.len() > self.max_visible
    }

    /// True when the currently-highlighted item has an `argument_hint`
    /// (so Enter should apply+space, not submit). v0.6 uses this for
    /// F008's single-Enter logic.
    pub fn selected_item_has_argument_hint(&self) -> bool {
        self.items
            .get(self.selected)
            .and_then(|i| i.argument_hint.as_ref())
            .is_some()
    }

    /// Move the selection up. Wraps around, and adjusts the scroll
    /// window so the selected item stays visible.
    pub fn select_up(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.items.len() - 1
        } else {
            self.selected - 1
        };
        self.scroll_into_view();
    }

    /// Move the selection down. Wraps around, and adjusts the scroll
    /// window so the selected item stays visible.
    pub fn select_down(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.items.len();
        self.scroll_into_view();
    }

    /// Make sure `self.selected` is in
    /// `[scroll_offset, scroll_offset + max_visible)`.
    pub fn scroll_into_view(&mut self) {
        if self.max_visible == 0 || self.items.len() <= self.max_visible {
            self.scroll_offset = 0;
            return;
        }
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + self.max_visible {
            self.scroll_offset = self.selected + 1 - self.max_visible;
        }
    }

    /// Return the currently selected item, if any.
    pub fn current(&self) -> Option<&CompletionItem> {
        self.items.get(self.selected)
    }

    /// Reset selection to top.
    pub fn reset(&mut self) {
        self.selected = 0;
    }
}

impl Default for CompletionPopup {
    fn default() -> Self {
        Self::new()
    }
}

/// Top-level app state. Render is a pure function of this.
///
/// `Clone` is hand-rolled because the `selector` field holds a
/// `Box<dyn SelectorState>`, which is not `Clone`. The default
/// clone (used by render snapshots, tests, etc.) drops the selector —
/// callers that need it must reconstruct it from current state.
#[derive(Debug)]
pub struct AppState {
    pub input: InputBuffer,
    pub transcript: Vec<TranscriptLine>,
    pub mode: RunMode,
    pub model: String,
    /// Current thinking level (e.g. "off", "medium", "high"). Updated
    /// by cycle_thinking and the /thinking command. Used as a
    /// cycle_thinking source of truth so we don't have to parse it
    /// out of the status string.
    pub thinking_level: Option<String>,
    /// Models available for cycling with Ctrl+P (loaded from
    /// `~/.pi/agent/models.json` or `models.json`). Empty means
    /// "use current model only".
    pub models_cycle: Vec<String>,
    /// Index of the currently-selected model in `models_cycle`. None if
    /// the current model isn't in the cycle (use models_cycle[0] as default).
    pub models_cycle_idx: Option<usize>,
    /// Path to the live settings.json (for `SettingsManager` writes).
    /// When `Some`, `cycle_model` / `cycle_thinking` / etc. will actually
    /// persist. When `None` (the default), changes are lost on exit.
    pub settings_path: Option<std::path::PathBuf>,
    /// Snapshot of `crate::settings::Settings` taken at app start.
    /// The auto-compaction trigger reads `reserve_tokens` from here.
    /// Settings are loaded from `~/.pi/agent/settings.json` if present;
    /// otherwise we use `Settings::default()`.
    pub settings_snapshot: crate::settings::Settings,
    pub session_id: Option<String>,
    pub tokens: TokenStats,
    /// Auto-scroll transcript to the bottom on new lines.
    pub autoscroll: bool,
    /// Number of lines scrolled up from the bottom (0 = at bottom).
    /// Increases when user presses PageUp; decreases on PageDown; resets
    /// to 0 when user is back at bottom and `autoscroll` is true.
    pub scroll_offset: usize,
    /// User-visible status (e.g., "running...", "ready").
    pub status: String,
    /// Current working directory for the status bar (e.g., "~/nini").
    /// `None` until the runtime populates it.
    pub cwd: Option<PathBuf>,
    /// Current git branch name, if any. `None` outside a git repo or
    /// before the runtime populates it. Mirrors Pi's footer.
    pub git_branch: Option<String>,
    /// Last edit-tool diff: (additions, deletions) in lines. Surfaced
    /// briefly in the status bar after each edit. `None` when no
    /// edit has been run yet, or when the user explicitly clears
    /// the indicator. Mirrors Pi's `[edit +N -M]` status pill.
    pub last_diff: Option<(usize, usize)>,
    /// Currently-loaded theme name. Mirrors settings.theme but kept
    /// here so the status bar can render the name without holding
    /// the settings lock. `None` means default (dark) theme.
    pub theme_name: Option<String>,
    /// Total estimated cost (USD) for the session, surfaced in the
    /// status bar when non-zero. Mirrors Pi's footer.
    pub cost_usd: f64,
    /// Provider's context-window size (tokens). Used to compute
    /// `context_percent` shown in the status bar. Mirrors Pi's
    /// `getContextUsage()` percent.
    pub context_window: u32,
    /// Last API-reported `usage.input` token count. Combined with
    /// `cache_read_tokens` and `context_window` to display a progress
    /// bar in the status bar.
    pub context_used: u32,
    /// Whether verbose debug logging is on (toggled by `/debug`). When
    /// true, the runtime appends per-keystroke lines to
    /// `~/.nini/state.log` so users can `tail -f` it.
    pub debug_logging: bool,
    /// When `Some`, the user pressed Ctrl+D and we're awaiting a
    /// second press within `QUIT_CONFIRM_WINDOW_MS` before actually
    /// quitting. Prevents accidental data loss.
    pub pending_quit: Option<std::time::Instant>,
    /// F020: When `true`, the user pressed Ctrl+G (or `/editor`)
    /// and the run loop should suspend the TUI and spawn the
    /// external editor on the current input buffer. The loop
    /// resets the flag once the editor dance completes (or
    /// errors out). Lives outside the key-event handler because
    /// the actual suspend/resume work needs `&mut Terminal`,
    /// which only the run loop holds.
    pub pending_external_editor: bool,
    /// Whether the F1 key was toggled on. The renderer swaps the bottom
    /// footer between a short hint set and an extended hint set so the
    /// user can see ALL key bindings without scrolling.
    pub help_extended: bool,
    /// Set by F1 to signal that the help selector should close on the
    /// next loop iteration (avoids the user having to press Esc).
    pub close_help: bool,
    /// Transcript search state. `Some` when the user has invoked `/`
    /// in normal (non-popup) editing mode; the runtime highlights all
    /// matches and supports n/N to jump between them.
    pub search: Option<SearchState>,
    /// Active selector panel (TreeSelector / SessionSelector / etc.).
    /// When `Some`, the runtime emits selector UI events on top of the
    /// transcript. Mirrors pi's selector stack.
    pub selector: Option<Box<dyn crate::selector::SelectorState + Send>>,
    /// Messages queued for the next turn (Pi "nextTurn" parity).
    /// These are user inputs received during compaction that should be
    /// injected as context alongside the next user prompt.
    /// Mirrors Pi's `_pendingNextTurnMessages`.
    pub pending_next_turn_messages: Vec<String>,
    /// True while auto-compaction is in progress. When true, /model
    /// /model/<arg> still works but the next prompt gets queued.
    pub is_compacting: bool,
    /// Abort signal: notify_waiters() aborts the currently running agent.
    /// Set by the AgentDriver when starting a turn, cleared on completion.
    /// v1: This is a coarse-grained abort (kills the agent task). For
    /// tool-level abort, use the AbortHandle from `nini_core::agent`.
    pub abort_signal: Option<Arc<tokio::sync::Notify>>,
    /// True while auto-compaction is in progress. When true, /model
    /// /model/<arg> still works but the next prompt gets queued.
    pub is_auto_compacting: bool,
    /// Optional completion popup (slash commands; extensible to `@file`).
    pub completion: Option<CompletionPopup>,
    /// Active session for persistence. When None, no session is active
    /// (e.g., --no-session mode). Arc+Mutex allows AgentSink (tokio task)
    /// to flush entries while the render loop holds a read handle.
    pub session: Option<Arc<Mutex<nini_session::Session>>>,
    /// Path to the session file on disk. Used for atomic write.
    pub session_path: Option<PathBuf>,
}

/// Transcript full-text search.
///
/// Invoked by pressing `/` while no slash-command popup is showing.
/// `matches` holds transcript line indices in match order; `current`
/// is the user's cursor into that vec (used by `n`/`N` jumps).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchState {
    /// The user's query (no leading `/`).
    pub query: String,
    /// Indices into the transcript that match `query`.
    pub matches: Vec<usize>,
    /// Cursor into `matches`. Wraps when pressing n/N past the ends.
    pub current: usize,
}

impl AppState {
    /// Update session-level metadata shown in the status bar.
    /// All fields are optional: pass `None` to clear.
    pub fn set_status_bar_metadata(
        &mut self,
        cwd: Option<PathBuf>,
        git_branch: Option<String>,
    ) {
        self.cwd = cwd;
        self.git_branch = git_branch;
    }

    /// Update cost + context usage shown in the status bar.
    pub fn set_status_bar_usage(
        &mut self,
        cost_usd: f64,
        context_window: u32,
        context_used: u32,
    ) {
        self.cost_usd = cost_usd;
        self.context_window = context_window;
        self.context_used = context_used;
    }

    /// Cheap clone for the render loop. Skips the `selector` field
    /// (Box<dyn SelectorState> doesn't implement Clone).
    pub fn clone_for_render(&self) -> Self {
        Self {
            input: self.input.clone(),
            transcript: self.transcript.clone(),
            mode: self.mode,
            model: self.model.clone(),
            session_id: self.session_id.clone(),
            status: self.status.clone(),
            tokens: self.tokens.clone(),
            models_cycle: self.models_cycle.clone(),
            models_cycle_idx: self.models_cycle_idx,
            settings_path: self.settings_path.clone(),
            settings_snapshot: self.settings_snapshot.clone(),
            thinking_level: self.thinking_level.clone(),
            pending_next_turn_messages: self.pending_next_turn_messages.clone(),
            is_auto_compacting: self.is_auto_compacting,
            abort_signal: self.abort_signal.clone(),
            scroll_offset: self.scroll_offset,
            is_compacting: self.is_compacting,
            cwd: self.cwd.clone(),
            git_branch: self.git_branch.clone(),
            last_diff: self.last_diff,
            cost_usd: self.cost_usd,
            context_window: self.context_window,
            theme_name: self.theme_name.clone(),
            context_used: self.context_used,
            debug_logging: self.debug_logging,
            pending_quit: self.pending_quit,
            pending_external_editor: self.pending_external_editor,
            help_extended: self.help_extended,
            close_help: self.close_help,
            search: self.search.clone(),
            autoscroll: self.autoscroll,
            completion: self.completion.clone(),
            session: self.session.clone(),
            session_path: self.session_path.clone(),
            selector: None,
        }
    }

    /// Deep clone. Excludes the `selector` field (Box<dyn> not Clone).
    /// Use `clone_for_render` for the common render path.
    pub fn clone_full(&self) -> Self {
        self.clone_for_render()
    }

    /// Hand-rolled `Clone` impl matching `clone_for_render`. Use this
    /// from generic code that calls `.clone()` on `AppState`.
    /// The selector panel is dropped (active selectors live on the
    /// runtime loop, not on cloned snapshots).
    pub fn clone(&self) -> Self {
        self.clone_for_render()
    }

    pub fn new(model: impl Into<String>) -> Self {
        Self::with_context_window(model.into(), 0)
    }

    /// Construct with an explicit context-window size. The runtime
    /// passes the value loaded from `~/.pi/agent/settings.json`; tests
    /// (and the no-config path) use the default constructor above.
    pub fn with_context_window(model: String, context_window: u32) -> Self {
        Self::with_settings(model, context_window, crate::settings::Settings::default())
    }

    /// Construct with an explicit context-window size AND a snapshot of
    /// the user-editable settings. The auto-compaction trigger reads
    /// `reserve_tokens` from the snapshot.
    pub fn with_settings(model: String, context_window: u32, settings: crate::settings::Settings) -> Self {
        Self {
            input: InputBuffer::new(),
            transcript: Vec::new(),
            mode: RunMode::Editing,
            model,
            session_id: None,
            tokens: TokenStats::default(),
            autoscroll: true,
            scroll_offset: 0,
            status: "ready".to_string(),
            cwd: None,
            git_branch: None,
            last_diff: None,
            cost_usd: 0.0,
            context_window,
            context_used: 0,
            debug_logging: false,
            pending_quit: None,
            pending_external_editor: false,
            help_extended: false,
            close_help: false,
            search: None,
            settings_snapshot: settings,
            completion: None,
            theme_name: None,
            session: None,
            session_path: None,
            models_cycle: Vec::new(),
            models_cycle_idx: None,
            settings_path: None,
            thinking_level: None,
            abort_signal: None,
            pending_next_turn_messages: Vec::new(),
            selector: None,
            is_compacting: false,
            is_auto_compacting: false,
        }
    }

    /// Initialize a new session for this working directory. Creates the session
    /// header and stores the file path for atomic writes.
    pub fn session_init(&mut self, path: PathBuf) {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let id = uuid::Uuid::now_v7().to_string();
        let session = nini_session::Session::new(cwd);
        self.session_id = Some(id);
        self.session = Some(Arc::new(Mutex::new(session)));
        self.session_path = Some(path);
    }

    /// Append a user or assistant message to the session. Call this BEFORE
    /// pushing to the transcript so the entry and transcript stay in sync.
    pub fn session_append(&mut self, parent_id: Option<String>, role: nini_core::Role, content: String) {
        let Some(ref arc) = self.session else { return };
        let msg = nini_core::AgentMessage {
            role,
            content: vec![nini_core::ContentBlock::Text { text: content }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        if let Ok(mut guard) = arc.try_lock() {
            let _id = guard.push_message(parent_id, msg);
        }
    }

    /// Flush pending entries to disk (atomic: temp file + rename).
    /// Logs errors but does not propagate — IO failures must not break the TUI.
    pub fn session_flush(&self) {
        let (path, session) = match (&self.session_path, &self.session) {
            (Some(p), Some(s)) => (p.clone(), s.clone()),
            _ => return,
        };
        // Spawn a blocking task so we don't block the render loop.
        std::thread::spawn(move || {
            if let Ok(guard) = session.try_lock() {
                if let Err(e) = guard.write_to_file(&path) {
                    eprintln!("[nini] session write error: {e}");
                }
            }
        });
    }

    /// Load a session from disk and replace the current session.
    pub fn session_load(&mut self, path: PathBuf) -> Result<(), nini_session::SessionError> {
        let session = nini_session::Session::read_from_file(&path)?;
        self.session_id = Some(session.header.id.clone());
        self.session = Some(Arc::new(Mutex::new(session)));
        self.session_path = Some(path);
        Ok(())
    }

    pub fn push_user(&mut self, text: impl Into<String>) {
        let s = text.into();
        self.transcript.push(TranscriptLine::User(s.clone()));
        self.session_append(None, nini_core::Role::User, s);
    }

    pub fn push_assistant(&mut self, text: impl Into<String>) {
        let s = text.into();
        self.push_assistant_raw(s.clone());
        self.session_append(None, nini_core::Role::Assistant, s);
    }

    /// Internal: push to transcript without session logging. Used by AgentSink
    /// (which handles its own session flush on TurnEnd).
    pub fn push_assistant_raw(&mut self, text: impl Into<String>) {
        self.transcript
            .push(TranscriptLine::AssistantText(text.into()));
    }

    pub fn push_tool_call(&mut self, name: impl Into<String>, args: impl Into<String>) {
        self.transcript.push(TranscriptLine::ToolCall {
            name: name.into(),
            args: args.into(),
            collapsed: false,
        });
    }

    pub fn push_tool_result(&mut self, ok: bool, content: impl Into<String>) {
        self.push_tool_result_raw(ok, content);
        // Tool results are part of the turn — session_append is handled on TurnEnd
        // by the session flush, so we do NOT append here to avoid double-logging.
    }

    /// Internal: push to transcript without session logging.
    pub fn push_tool_result_raw(&mut self, ok: bool, content: impl Into<String>) {
        self.transcript.push(TranscriptLine::ToolResult {
            ok,
            content: content.into(),
            collapsed: false,
        });
    }

    pub fn push_divider(&mut self) {
        self.transcript.push(TranscriptLine::Divider);
    }

    pub fn transcript_len(&self) -> usize {
        self.transcript.len()
    }


    /// Cheap heuristic: estimate the transcript's token usage and decide
    /// whether auto-compaction should fire before the next user prompt.
    ///
    /// Uses `(text_chars + 3) / 4` per line — a well-known approximation
    /// for English (~4 chars per BPE token). Multi-byte chars (CJK) are
    /// counted as 1 unit so the estimate is conservative for non-English
    /// text. This is the same formula `estimate_string_tokens` applies in
    /// the local heuristic summarizer.
    ///
    /// Returns `true` when estimated > `context_window - reserve_tokens`.
    pub fn should_auto_compact(&self, settings: &crate::settings::Settings) -> bool {
        if self.context_window == 0 {
            return false;
        }
        let budget = self
            .context_window
            .saturating_sub(settings.reserve_tokens);
        let estimated = self.estimate_transcript_tokens();
        estimated > budget
    }

    /// Sum of (chars / 4) across every text-bearing transcript line.
    /// Used by [`should_auto_compact`] and shown in the status bar.
    pub fn estimate_transcript_tokens(&self) -> u32 {
        let mut total: u32 = 0;
        for line in &self.transcript {
            use crate::state::TranscriptLine;
            match line {
                TranscriptLine::User(s) => total += chars_to_tokens(s),
                TranscriptLine::AssistantText(s) => total += chars_to_tokens(s),
                TranscriptLine::ToolCall { name, args, .. } => {
                    total += chars_to_tokens(name) + chars_to_tokens(args);
                }
                TranscriptLine::ToolResult { content, .. } => total += chars_to_tokens(content),
                TranscriptLine::BashExecution { cmd, output, .. } => {
                    total += chars_to_tokens(cmd) + chars_to_tokens(output);
                }
                TranscriptLine::Divider => {}
            }
        }
        total
    }

    /// Apply a deterministic local compaction: take the older half of
    /// the transcript (every line whose index is < cut_at), replace it
    /// with a single AssistantText prefix summarising it via
    /// `nini_core::compaction::generate_local_summary`. Returns the
    /// number of transcript lines that were folded.
    ///
    /// The runtime invokes this from the auto-compaction path when the
    /// provider cannot be reached (or when running in `--no-llm` mode).
    pub fn auto_compact_local(&mut self) -> usize {
        use crate::state::TranscriptLine;
        let total = self.transcript.len();
        if total < 4 {
            return 0;
        }
        let cut_at = total / 2;
        let prefix_lines: Vec<TranscriptLine> = self.transcript.drain(..cut_at).collect();
        // Build a short textual summary from the prefix. We don't have
        // direct access to `nini_core::Entry` from the prefix, but
        // `generate_local_summary` works on `Vec<Entry>` — so we
        // synthesise a flat string from the prefix lines instead. This
        // keeps the TUI-side compaction self-contained.
        let mut summary = String::from("# Compaction Summary\n\n");
        for (i, line) in prefix_lines.iter().enumerate() {
            summary.push_str(&format!("{i:>3}. {}\n", line_summary_text(line)));
        }
        summary.push_str(&format!("\n({} entries folded into this summary.)\n", prefix_lines.len()));
        // Replace prefix with a single AssistantText.
        self.transcript
            .insert(0, TranscriptLine::AssistantText(format!(
                "[CONTEXT SUMMARY]\n\n{summary}"
            )));
        prefix_lines.len()
    }


    /// Toggle the `collapsed` flag on the transcript line at `index` if
    /// that line is collapsible (ToolCall / ToolResult / BashExecution).
    /// Returns true if the toggle changed the line's state.
    pub fn toggle_collapsed(&mut self, index: usize) -> bool {
        if let Some(line) = self.transcript.get_mut(index) {
            match line {
                TranscriptLine::ToolCall { collapsed, .. }
                | TranscriptLine::ToolResult { collapsed, .. }
                | TranscriptLine::BashExecution { collapsed, .. } => {
                    *collapsed = !*collapsed;
                    true
                }
                _ => false,
            }
        } else {
            false
        }
    }

    /// Collapse every collapsible line in the transcript. Used by
    /// `/compact-output` (future slash) or `:fold-all` (future keystroke).
    /// Returns the number of lines collapsed.
    pub fn collapse_all(&mut self) -> usize {
        let mut n = 0;
        for line in self.transcript.iter_mut() {
            if let TranscriptLine::ToolCall { collapsed, .. }
            | TranscriptLine::ToolResult { collapsed, .. }
            | TranscriptLine::BashExecution { collapsed, .. } = line
            {
                if !*collapsed {
                    *collapsed = true;
                    n += 1;
                }
            }
        }
        n
    }

    /// Update the completion popup from the current input. Hides if no
    /// completions apply. Handles both `/` (slash-command) and `@`
    /// (file-path) prefixes.
    pub fn refresh_completion(&mut self) {
        // Slash-command completion takes priority.
        if let Some(prefix) = self.input.slash_prefix() {
            let items: Vec<CompletionItem> = crate::commands::complete(prefix, 8)
                .into_iter()
                .map(|def| CompletionItem {
                    name: def.name.to_string(),
                    description: def.description.to_string(),
                    argument_hint: def.argument_hint.map(|s| s.to_string()),
                })
                .collect();
            if items.is_empty() {
                self.completion = None;
            } else {
                let new_names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
                let prev_selected_name = self
                    .completion
                    .as_ref()
                    .and_then(|p| p.current())
                    .map(|i| i.name.clone());
                let new_selection = prev_selected_name
                    .as_ref()
                    .and_then(|n| new_names.iter().position(|m| m == n))
                    .unwrap_or(0);
                self.completion = Some(CompletionPopup {
                    items,
                    selected: new_selection,
                    scroll_offset: 0,
                    max_visible: DEFAULT_COMPLETION_VISIBLE,
                });
            }
            return;
        }
        // @ file-path completion.
        if let Some(path_prefix) = crate::file_completion::extract_at_prefix(
            &self.input.text,
            self.input.cursor,
        ) {
            let cwd = std::env::current_dir().unwrap_or_default();
            let files = crate::file_completion::search_files_as_struct(&cwd, &path_prefix);
            if files.is_empty() {
                self.completion = None;
                return;
            }
            let items: Vec<CompletionItem> = files
                .into_iter()
                .map(|f| {
                    let mut name = f.relative_path.clone();
                    if f.is_dir {
                        name.push('/');
                    }
                    CompletionItem {
                        name,
                        description: f.relative_path.clone(),
                        argument_hint: if f.is_dir {
                            Some("dir".to_string())
                        } else {
                            Some("file".to_string())
                        },
                    }
                })
                .collect();
            self.completion = Some(CompletionPopup {
                items,
                selected: 0,
                scroll_offset: 0,
                max_visible: DEFAULT_COMPLETION_VISIBLE,
            });
            return;
        }
        self.completion = None;
    }

    /// Apply the currently-selected completion to the input buffer.
    /// Replaces the partial command name with the full one.
    pub fn apply_completion(&mut self) {
        let Some(popup) = &self.completion else {
            return;
        };
        let Some(item) = popup.current() else {
            return;
        };
        let full = format!("/{}", item.name);
        // Replace the input with the full command, leaving the cursor at end.
        self.input.text = full;
        self.input.cursor = self.input.text.len();
        // Add a space if the command takes arguments.
        if item.argument_hint.is_some() {
            self.input.text.push(' ');
            self.input.cursor += 1;
        }
        self.completion = None;
    }

    /// Begin a transcript search. Called when the user presses `/` in
    /// editing mode (with no slash-command popup showing).
    pub fn begin_search(&mut self) {
        self.search = Some(SearchState {
            query: String::new(),
            matches: Vec::new(),
            current: 0,
        });
    }

    /// Update the search query and recompute matches against the
    /// current transcript. Case-insensitive substring match.
    pub fn update_search_query(&mut self, q: String) {
        let Some(s) = self.search.as_mut() else { return };
        s.query = q.clone();
        let q_lower = q.to_lowercase();
        let mut new_matches: Vec<usize> = Vec::new();
        for (i, line) in self.transcript.iter().enumerate() {
            let text = line.summary_text().to_lowercase();
            if !q_lower.is_empty() && text.contains(&q_lower) {
                new_matches.push(i);
            }
        }
        s.matches = new_matches;
        if s.matches.is_empty() {
            s.current = 0;
        } else if s.current >= s.matches.len() {
            s.current = s.matches.len() - 1;
        }
    }

    /// Advance to the next match (wraps at the end).
    pub fn search_next(&mut self) {
        if let Some(s) = self.search.as_mut() {
            if !s.matches.is_empty() {
                s.current = (s.current + 1) % s.matches.len();
            }
        }
    }

    /// Advance to the previous match (wraps at the start).
    pub fn search_prev(&mut self) {
        if let Some(s) = self.search.as_mut() {
            if !s.matches.is_empty() {
                s.current = if s.current == 0 {
                    s.matches.len() - 1
                } else {
                    s.current - 1
                };
            }
        }
    }

    /// Close the search overlay and clear highlights.
    pub fn end_search(&mut self) {
        self.search = None;
    }
}


fn chars_to_tokens(s: &str) -> u32 {
    ((s.chars().count() as u32) + 3) / 4
}

fn line_summary_text(line: &crate::state::TranscriptLine) -> String {
    line.summary_text()
}

/// Public summary form of a transcript line, used by the transcript
/// search (F019) to do case-insensitive substring matches. We
/// intentionally include prefixes like `[user]` so users searching
/// for "tool-call" can find the right lines without guessing the
/// underlying text.
pub(crate) fn line_summary_text_impl(line: &crate::state::TranscriptLine) -> String {
    use crate::state::TranscriptLine;
    match line {
        TranscriptLine::User(s) => format!("[user] {}", s.replace('\n', " ")),
        TranscriptLine::AssistantText(s) => format!("[assistant] {}", truncate(s, 60)),
        TranscriptLine::ToolCall { name, args, .. } => {
            format!("[tool-call] {name}({})", truncate(args, 40))
        }
        TranscriptLine::ToolResult { content, .. } => format!("[tool-result] {}", truncate(content, 60)),
        TranscriptLine::BashExecution { cmd, output, .. } => {
            format!("[bash] {cmd} -> {}", truncate(output, 40))
        }
        TranscriptLine::Divider => "[divider]".to_string(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_backspace() {
        let mut buf = InputBuffer::new();
        buf.insert_char('h');
        buf.insert_char('i');
        assert_eq!(buf.text, "hi");
        assert_eq!(buf.cursor, 2);
        buf.backspace();
        assert_eq!(buf.text, "h");
        assert_eq!(buf.cursor, 1);
    }

    #[test]
    fn cursor_movement() {
        let mut buf = InputBuffer::new();
        buf.insert_str("hello");
        buf.move_to_start();
        assert_eq!(buf.cursor, 0);
        buf.move_right();
        assert_eq!(buf.cursor, 1);
        buf.move_word_right();
        // After 'hello' with no trailing whitespace, word_right jumps to end
        assert_eq!(buf.cursor, 5);
    }

    #[test]
    fn word_left_from_middle() {
        let mut buf = InputBuffer::new();
        buf.insert_str("hello world");
        buf.cursor = 11;
        buf.move_word_left();
        // Should land at position 6 (after 'hello ')
        assert_eq!(buf.cursor, 6);
    }

    #[test]
    fn kill_to_line_end() {
        let mut buf = InputBuffer::new();
        buf.insert_str("hello world");
        buf.cursor = 5;
        buf.kill_to_line_end();
        assert_eq!(buf.text, "hello");
    }

    #[test]
    fn submit_archives_to_history() {
        let mut buf = InputBuffer::new();
        buf.insert_str("first message");
        let submitted = buf.submit();
        assert_eq!(submitted, "first message");
        assert!(buf.text.is_empty());
        assert_eq!(buf.history.len(), 1);
        assert_eq!(buf.history[0], "first message");
    }

    #[test]
    fn empty_submit_does_not_archive() {
        let mut buf = InputBuffer::new();
        buf.insert_str("   ");
        let submitted = buf.submit();
        assert_eq!(submitted, "   ");
        assert!(buf.history.is_empty());
    }

    #[test]
    fn history_recall_round_trip() {
        let mut buf = InputBuffer::new();
        buf.insert_str("first");
        buf.submit();
        buf.insert_str("second");
        buf.submit();
        // Cursor in editing. Up → older.
        buf.recall_history(-1);
        assert_eq!(buf.text, "second");
    }

    #[test]
    fn kill_ring_captures_killed_text() {
        let mut buf = InputBuffer::new();
        buf.insert_str("hello ");
        buf.cursor = 6; // after "hello "
        buf.insert_str("world");
        buf.cursor = 6; // mid-string
        buf.kill_to_line_end();
        assert_eq!(buf.text, "hello ");
        assert_eq!(buf.kill_ring, vec!["world"]);
    }

    #[test]
    fn yank_restores_killed_text() {
        let mut buf = InputBuffer::new();
        buf.insert_str("hello ");
        buf.cursor = 6;
        buf.insert_str("world");
        buf.cursor = 6;
        buf.kill_to_line_end();
        assert_eq!(buf.text, "hello ");
        assert!(buf.yank());
        assert_eq!(buf.text, "hello world");
    }

    #[test]
    fn yank_with_empty_ring_returns_false() {
        let mut buf = InputBuffer::new();
        assert!(!buf.yank());
        assert_eq!(buf.text, "");
    }

    #[test]
    fn kill_word_forward_kills_to_next_word() {
        let mut buf = InputBuffer::new();
        buf.insert_str("hello world foo");
        buf.cursor = 0;
        buf.kill_word_forward();
        assert_eq!(buf.text, " world foo");
        assert_eq!(buf.kill_ring, vec!["hello"]);
    }

    #[test]
    fn undo_reverts_last_edit() {
        let mut buf = InputBuffer::new();
        buf.insert_char('a');
        buf.insert_char('b');
        buf.insert_char('c');
        assert_eq!(buf.text, "abc");
        assert!(buf.undo());
        assert_eq!(buf.text, "ab");
        assert!(buf.undo());
        assert_eq!(buf.text, "a");
        assert!(buf.undo());
        assert_eq!(buf.text, "");
        // Empty stack: 4th undo returns false.
        assert!(!buf.undo());
    }

    #[test]
    fn history_recall_full_cycle() {
        let mut buf = InputBuffer::new();
        buf.insert_str("first");
        buf.submit();
        buf.insert_str("second");
        buf.submit();
        // Cursor in editing. Up → older.
        buf.recall_history(-1);
        assert_eq!(buf.text, "second");
    }

    #[test]
    fn push_and_count() {
        let mut s = AppState::new("test-model");
        s.push_user("hi");
        s.push_assistant("hello");
        s.push_tool_call("bash", "{}");
        s.push_tool_result(true, "ok");
        s.push_divider();
        assert_eq!(s.transcript_len(), 5);
    }

    #[test]
    fn toggle_collapsed_flips_tool_call_state() {
        let mut s = AppState::new("test");
        s.push_tool_call("bash", "{}");
        // ToolCall index is 0.
        assert!(s.toggle_collapsed(0));
        // Verify the line is now collapsed by re-pushing another line
        // and reading back the transcript — collapsed flag persists.
        s.push_divider();
        match &s.transcript[0] {
            TranscriptLine::ToolCall { collapsed, .. } => assert!(*collapsed),
            other => panic!("expected ToolCall, got {other:?}"),
        }
        // Toggle back to expanded.
        assert!(s.toggle_collapsed(0));
        match &s.transcript[0] {
            TranscriptLine::ToolCall { collapsed, .. } => assert!(!(*collapsed)),
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn toggle_collapsed_flips_tool_result_and_bash() {
        let mut s = AppState::new("test");
        s.push_tool_result(true, "ok");
        s.push_tool_call("read", "{}");
        // Append a bash via the underlying TranscriptLine constructor
        // since we don't have a public push_bash helper yet.
        s.transcript.push(TranscriptLine::BashExecution {
            id: "b1".to_string(),
            cmd: "ls".to_string(),
            output: "file1\nfile2".to_string(),
            stderr: String::new(),
            ok: true,
            exit_code: Some(0),
            duration_ms: 5,
            collapsed: false,
        });
        // Indexes: 0=ToolResult, 1=ToolCall, 2=BashExecution.
        assert!(s.toggle_collapsed(0));
        assert!(s.toggle_collapsed(1));
        assert!(s.toggle_collapsed(2));
        match &s.transcript[0] {
            TranscriptLine::ToolResult { collapsed, .. } => assert!(*collapsed),
            _ => panic!(),
        }
        match &s.transcript[1] {
            TranscriptLine::ToolCall { collapsed, .. } => assert!(*collapsed),
            _ => panic!(),
        }
        match &s.transcript[2] {
            TranscriptLine::BashExecution { collapsed, .. } => assert!(*collapsed),
            _ => panic!(),
        }
    }

    #[test]
    fn toggle_collapsed_returns_false_on_user_or_assistant_line() {
        let mut s = AppState::new("test");
        s.push_user("hi");
        s.push_assistant("hello");
        // Neither user nor assistant lines are collapsible.
        assert!(!s.toggle_collapsed(0));
        assert!(!s.toggle_collapsed(1));
    }

    #[test]
    fn toggle_collapsed_returns_false_on_out_of_bounds() {
        let mut s = AppState::new("test");
        s.push_tool_call("bash", "{}");
        assert!(!s.toggle_collapsed(99));
    }


    #[test]
    fn estimate_transcript_tokens_sums_text_lines() {
        let mut s = AppState::new("m");
        s.push_user("hello");   // 5 chars -> 2 tokens
        s.push_assistant("world this is longer");  // 21 -> 6 tokens
        s.push_tool_result(true, "out");
        // total chars / 4 rounded up
        assert!(s.estimate_transcript_tokens() > 0);
    }

    #[test]
    fn should_auto_compact_returns_false_when_window_zero() {
        // No context_window set: never auto-compact (user hasn't
        // configured a model size yet).
        let mut s = AppState::new("m");
        s.context_window = 0;
        s.push_user("a".repeat(10_000));
        let settings = crate::settings::Settings::default();
        assert!(!s.should_auto_compact(&settings));
    }

    #[test]
    fn should_auto_compact_returns_true_over_budget() {
        // 1000-token window with 200 reserve; a 4000-char message
        // estimates at ~1000 tokens which exceeds 800 budget.
        let mut s = AppState::with_context_window("m".into(), 1_000);
        let mut settings = crate::settings::Settings::default();
        settings.reserve_tokens = 200;
        s.push_user("a".repeat(4_000));
        assert!(s.should_auto_compact(&settings));
    }

    #[test]
    fn should_auto_compact_returns_false_under_budget() {
        let mut s = AppState::with_context_window("m".into(), 100_000);
        let settings = crate::settings::Settings::default();
        s.push_user("hello");
        assert!(!s.should_auto_compact(&settings));
    }

    #[test]
    fn auto_compact_local_replaces_prefix_with_summary() {
        let mut s = AppState::new("m");
        for i in 0..8 {
            s.push_user(format!("user message {i}"));
        }
        let before = s.transcript.len();
        let folded = s.auto_compact_local();
        assert!(folded >= 4, "should fold at least half");
        assert!(s.transcript.len() < before, "transcript should shrink");
        // The new head should be a CONTEXT SUMMARY line.
        match &s.transcript[0] {
            TranscriptLine::AssistantText(t) => assert!(t.starts_with("[CONTEXT SUMMARY]")),
            other => panic!("expected AssistantText head, got {other:?}"),
        }
    }

    #[test]
    fn auto_compact_local_short_transcript_is_noop() {
        let mut s = AppState::new("m");
        s.push_user("hi");
        let folded = s.auto_compact_local();
        assert_eq!(folded, 0);
        assert_eq!(s.transcript.len(), 1);
    }

    #[test]
    fn collapse_all_folds_every_collapsible_line() {
        let mut s = AppState::new("test");
        s.push_tool_call("bash", "{}");
        s.push_tool_result(true, "ok");
        s.push_divider();
        s.push_user("hi");
        s.push_assistant("hello");
        // Index 5: a bash via direct TranscriptLine.
        s.transcript.push(TranscriptLine::BashExecution {
            id: "b2".to_string(),
            cmd: "ls".to_string(),
            output: "out".to_string(),
            stderr: String::new(),
            ok: true,
            exit_code: Some(0),
            duration_ms: 1,
            collapsed: false,
        });
        let folded = s.collapse_all();
        // 3 collapsible lines were folded.
        assert_eq!(folded, 3);
        // After collapse_all, all collapsibles are collapsed.
        for (i, line) in s.transcript.iter().enumerate() {
            match line {
                TranscriptLine::ToolCall { collapsed, .. }
                | TranscriptLine::ToolResult { collapsed, .. }
                | TranscriptLine::BashExecution { collapsed, .. } => {
                    assert!(*collapsed, "index {i} should be collapsed");
                }
                _ => {}
            }
        }
        // Calling collapse_all again folds zero new lines.
        assert_eq!(s.collapse_all(), 0);
    }

    // ============================================================
    // F019 transcript search
    // ============================================================
    #[test]
    fn search_finds_case_insensitive_substring_matches() {
        let mut s = AppState::new("test");
        s.push_user("hello WORLD");
        s.push_assistant("hi world");
        s.push_user("goodbye");
        s.begin_search();
        s.update_search_query("world".to_string());
        let search = s.search.as_ref().expect("search active");
        // Two lines mention "world" (case-insensitive).
        assert_eq!(search.matches.len(), 2);
    }

    #[test]
    fn search_empty_query_yields_no_matches() {
        let mut s = AppState::new("test");
        s.push_user("hello");
        s.begin_search();
        // Empty query matches nothing (matches is a snapshot of "what
        // does the user want highlighted"; nothing).
        s.update_search_query(String::new());
        assert!(s.search.as_ref().unwrap().matches.is_empty());
    }

    #[test]
    fn search_next_advances_with_wrap() {
        let mut s = AppState::new("test");
        for i in 0..3 {
            s.push_user(format!("line {i} match"));
        }
        s.begin_search();
        s.update_search_query("match".to_string());
        assert_eq!(s.search.as_ref().unwrap().matches.len(), 3);
        // Initially at index 0; search_next → 1 → 2 → wraps to 0.
        s.search_next();
        assert_eq!(s.search.as_ref().unwrap().current, 1);
        s.search_next();
        assert_eq!(s.search.as_ref().unwrap().current, 2);
        s.search_next();
        assert_eq!(s.search.as_ref().unwrap().current, 0);
    }

    #[test]
    fn search_prev_decrements_with_wrap() {
        let mut s = AppState::new("test");
        for i in 0..3 {
            s.push_user(format!("line {i} hit"));
        }
        s.begin_search();
        s.update_search_query("hit".to_string());
        // current=0; search_prev wraps to last.
        s.search_prev();
        assert_eq!(s.search.as_ref().unwrap().current, 2);
    }

    #[test]
    fn end_search_clears_state() {
        let mut s = AppState::new("test");
        s.push_user("hello");
        s.begin_search();
        s.update_search_query("hello".to_string());
        s.end_search();
        assert!(s.search.is_none());
    }

    // ============================================================
    // F016 undo + kill ring end-to-end verification
    // ============================================================
    #[test]
    fn kill_word_backward_drains_word() {
        let mut s = AppState::new("test");
        for c in "hello world".chars() {
            s.input.insert_char(c);
        }
        // Cursor at end (after "world"). kill_word_backward should
        // remove "world " (the trailing word + space).
        s.input.kill_word_backward();
        assert_eq!(s.input.text, "hello");
    }

    #[test]
    fn kill_word_forward_drains_word() {
        let mut s = AppState::new("test");
        for c in "hello world".chars() {
            s.input.insert_char(c);
        }
        // Move to start, then kill forward — removes "hello" (the
        // word itself; the trailing space stays).
        s.input.move_to_start();
        s.input.kill_word_forward();
        assert_eq!(s.input.text, " world");
    }

    #[test]
    fn kill_word_forward_from_middle_kills_one_word() {
        // Alt+D semantics: kill forward by one word. From the middle
        // of "def", should kill the rest of "def" — not " ghi".
        let mut s = AppState::new("test");
        for c in "abc def ghi".chars() {
            s.input.insert_char(c);
        }
        // Cursor at index 5 is between 'd' and 'e' of "def".
        s.input.cursor = 5;
        s.input.kill_word_forward();
        // Should remove "ef" (rest of current word); the " ghi"
        // remains.
        assert_eq!(s.input.text, "abc d ghi");
    }

    #[test]
    fn yank_restores_last_kill() {
        let mut s = AppState::new("test");
        for c in "hello world".chars() {
            s.input.insert_char(c);
        }
        s.input.move_to_start();
        s.input.kill_word_forward(); // kills "hello"
        assert_eq!(s.input.text, " world");
        // Yank restores "hello" at the cursor position.
        let yanked = s.input.yank();
        assert!(yanked);
        assert_eq!(s.input.text, "hello world");
    }

    #[test]
    fn undo_restores_pre_edit_text() {
        let mut s = AppState::new("test");
        for c in "abc".chars() {
            s.input.insert_char(c);
        }
        assert_eq!(s.input.text, "abc");
        // Ctrl+Z undoes the last char.
        s.input.undo();
        assert_eq!(s.input.text, "ab");
        s.input.undo();
        assert_eq!(s.input.text, "a");
    }

    #[test]
    fn kill_to_line_end_then_undo_roundtrip() {
        // Ctrl+K kills from cursor to end of line; undo should restore.
        let mut s = AppState::new("test");
        for c in "abcdef".chars() {
            s.input.insert_char(c);
        }
        s.input.move_to_start();
        s.input.kill_to_line_end();
        assert_eq!(s.input.text, "");
        s.input.undo();
        assert_eq!(s.input.text, "abcdef");
    }
}
