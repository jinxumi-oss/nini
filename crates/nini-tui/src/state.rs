//! App state for the TUI.
//!
//! The state is a small, observable model: prompt input + cursor, conversation
//! log (transcript lines), run mode (editing vs. running vs. aborted),
//! and minimal model/session metadata. Render is a pure function of state.
//!
//! All editor mutations are pure functions on `InputBuffer`, which makes
//! them trivially testable without a terminal.

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
    },
    /// Tool result. `Ok`/`Err` reflects `ToolOutput.is_error`.
    ToolResult {
        ok: bool,
        content: String,
    },
    /// System-injected divider (turn boundary).
    Divider,
}

impl TranscriptLine {
    /// If this line is `AssistantText`, return a reference to the text.
    pub fn as_assistant_text(&self) -> Option<&str> {
        match self {
            TranscriptLine::AssistantText(s) => Some(s.as_str()),
            _ => None,
        }
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
        // Insert at cursor (byte position). Caller must ensure char boundary.
        let bytes = c.len_utf8();
        let pos = self.cursor.min(self.text.len());
        self.text.insert(pos, c);
        self.cursor = pos + bytes;
    }

    pub fn insert_str(&mut self, s: &str) {
        let pos = self.cursor.min(self.text.len());
        self.text.insert_str(pos, s);
        self.cursor += s.len();
    }

    /// Backspace: delete the char before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
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
    /// `bash` Alt+Left behavior.
    pub fn move_word_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let bytes = self.text.as_bytes();
        let mut i = self.cursor;
        // Phase 1: skip whitespace before cursor (if any).
        while i > 0 && (bytes[i - 1] as char).is_whitespace() {
            i -= 1;
        }
        // Phase 2: skip non-whitespace.
        while i > 0 && !(bytes[i - 1] as char).is_whitespace() {
            i -= 1;
        }
        self.cursor = i;
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
        self.text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
    }

    pub fn kill_to_line_end(&mut self) {
        self.text.truncate(self.cursor);
    }

    pub fn kill_word_backward(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let s = &self.text[..self.cursor];
        let trimmed = s.trim_end_matches(|c: char| !c.is_whitespace());
        let prev = trimmed.trim_end_matches(|c: char| c.is_whitespace()).len();
        self.text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.history_cursor = None;
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
    /// Currently selected item index (highlighted). 0-based.
    pub selected: usize,
}

/// One entry in the completion popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub name: String,
    pub description: String,
    /// Optional argument hint shown in dim text after the name.
    pub argument_hint: Option<String>,
}

impl CompletionPopup {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            selected: 0,
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

    /// Move the selection up. Wraps around.
    pub fn select_up(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.items.len() - 1
        } else {
            self.selected - 1
        };
    }

    /// Move the selection down. Wraps around.
    pub fn select_down(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.items.len();
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
#[derive(Debug, Clone)]
pub struct AppState {
    pub input: InputBuffer,
    pub transcript: Vec<TranscriptLine>,
    pub mode: RunMode,
    pub model: String,
    pub session_id: Option<String>,
    pub tokens: TokenStats,
    /// Auto-scroll transcript to the bottom on new lines.
    pub autoscroll: bool,
    /// User-visible status (e.g., "running...", "ready").
    pub status: String,
    /// Optional completion popup (slash commands; extensible to `@file`).
    pub completion: Option<CompletionPopup>,
}

impl AppState {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            input: InputBuffer::new(),
            transcript: Vec::new(),
            mode: RunMode::Editing,
            model: model.into(),
            session_id: None,
            tokens: TokenStats::default(),
            autoscroll: true,
            status: "ready".to_string(),
            completion: None,
        }
    }

    pub fn push_user(&mut self, text: impl Into<String>) {
        self.transcript.push(TranscriptLine::User(text.into()));
    }

    pub fn push_assistant(&mut self, text: impl Into<String>) {
        self.transcript
            .push(TranscriptLine::AssistantText(text.into()));
    }

    pub fn push_tool_call(&mut self, name: impl Into<String>, args: impl Into<String>) {
        self.transcript.push(TranscriptLine::ToolCall {
            name: name.into(),
            args: args.into(),
        });
    }

    pub fn push_tool_result(&mut self, ok: bool, content: impl Into<String>) {
        self.transcript.push(TranscriptLine::ToolResult {
            ok,
            content: content.into(),
        });
    }

    pub fn push_divider(&mut self) {
        self.transcript.push(TranscriptLine::Divider);
    }

    pub fn transcript_len(&self) -> usize {
        self.transcript.len()
    }

    /// Update the completion popup from the current input. Hides if no
    /// completions apply.
    pub fn refresh_completion(&mut self) {
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
                // Keep selection if the same item is still present, else reset.
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
                });
            }
        } else {
            self.completion = None;
        }
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
        assert_eq!(buf.cursor, 6);
        buf.recall_history(-1);
        assert_eq!(buf.text, "first");
        // Down → newer
        buf.recall_history(1);
        assert_eq!(buf.text, "second");
        buf.recall_history(1);
        assert!(buf.text.is_empty());
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
}
