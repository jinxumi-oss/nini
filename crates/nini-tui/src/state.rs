//! App state for the TUI.
//!
//! The state is a small, observable model: prompt input + cursor, conversation
//! log (transcript lines), run mode (editing vs. running vs. aborted),
//! and minimal model/session metadata. Render is a pure function of state.
//!
//! All editor mutations are pure functions on `InputBuffer`, which makes
//! them trivially testable without a terminal.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
// v0.10 (fix): this file imported `tokio::sync::Mutex` for `session`
// because a comment claimed "`session` is held across `.await` points in
// the agent task, so it needs the async mutex". That claim is false, and
// it is the reason a normal race could become silent data loss: every one
// of the eleven session access points is a short synchronous critical
// section in an ordinary `fn` (`session_append_message`, `session_compact`,
// `session_context_messages`, `/fork`, `/tree`, the flush thread), and the
// workspace contains exactly one `.lock().await` — `nini-tools`
// `mutation_queue`, a different crate and a different lock.
//
// `tokio::sync::Mutex` is for a guard that lives across `.await`. Used for
// a guard that never does, it invites `try_lock`, and `try_lock` turns
// ordinary contention into "no context", "no message", "no tree" — all
// reported as success. `std::sync::Mutex` keeps every one of those paths
// waiting, which is what the code always meant. The render cache below was
// already `StdMutex` for the same reason.
use unicode_segmentation::UnicodeSegmentation;

/// One row of the conversation transcript. Either a user message, assistant
/// text, a tool call (with tool name + JSON args), or a tool result.
/// Render a session's live context into transcript lines.
///
/// A compaction entry becomes one `AssistantText` line carrying the
/// `CONTEXT SUMMARY` marker, which is the same text
/// `nini_core::conversion` feeds the model, so what the user reads and what
/// the model sees say the same thing.
fn render_context_entries(entries: &[nini_core::SessionEntry]) -> Vec<TranscriptLine> {
    use nini_core::entries::{AgentMessage, ContentBlock, StringOrContentBlocks};
    let mut lines = Vec::new();

    let text_of = |content: &StringOrContentBlocks| -> Option<String> {
        match content {
            StringOrContentBlocks::String(t) => Some(t.clone()),
            StringOrContentBlocks::Blocks(bs) => bs.iter().find_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            }),
        }
    };

    for entry in entries {
        // A compaction is its own entry type, not a message. It renders as
        // one line carrying the same `CONTEXT SUMMARY` marker the model is
        // given, so what the user reads and what the model sees agree.
        if let nini_core::SessionEntry::Compaction(c) = entry {
            lines.push(TranscriptLine::AssistantText(
                nini_core::compaction::context_summary_message(&c.summary),
            ));
            continue;
        }
        let nini_core::SessionEntry::Message(m) = entry else {
            continue;
        };
        match &m.message {
            AgentMessage::User(u) => {
                if let Some(t) = text_of(&u.content) {
                    lines.push(TranscriptLine::User(t));
                }
            }
            AgentMessage::Assistant(a) => {
                // v0.10 (R2h): a failed turn has no content blocks, so
                // without this it rendered as nothing at all - a resumed
                // session could not tell it had happened.
                if a.stop_reason == nini_core::entries::StopReason::Error
                    || a.stop_reason == nini_core::entries::StopReason::Aborted
                {
                    let detail = a
                        .error_message
                        .clone()
                        .unwrap_or_else(|| "Unknown error".to_string());
                    lines.push(TranscriptLine::StopNotice(format!("Error: {detail}")));
                }
                for b in &a.content {
                    match b {
                        ContentBlock::Text { text } => {
                            lines.push(TranscriptLine::AssistantText(text.clone()))
                        }
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                        } => lines.push(TranscriptLine::ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            args: arguments.to_string(),
                            collapsed: false,
                        }),
                        _ => {}
                    }
                }
            }
            AgentMessage::ToolResult(t) => {
                let body = t
                    .content
                    .iter()
                    .find_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                lines.push(TranscriptLine::ToolResult {
                    id: t.tool_call_id.clone(),
                    name: t.tool_name.clone(),
                    ok: !t.is_error,
                    content: body,
                    collapsed: false,
                    duration_ms: None,
                });
            }
            AgentMessage::BashExecution(b) => {
                // v0.10 (R2f): previously fell into the `_` arm and was
                // dropped, so a resumed session could not show a command the
                // user had run.
                lines.push(TranscriptLine::BashExecution {
                    id: String::new(),
                    cmd: b.command.clone(),
                    output: b.output.clone(),
                    stderr: String::new(),
                    ok: b.exit_code == Some(0),
                    exit_code: b.exit_code,
                    duration_ms: 0,
                    collapsed: false,
                });
            }
            _ => {}
        }
    }
    lines
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptLine {
    User(String),
    AssistantText(String),
    /// v0.8: model reasoning (inside `<think>...</think>`).
    /// Rendered with dim/italic style (Pi-style).
    ThinkingText(String),
    /// Tool invocation. `args` is rendered as a one-line JSON preview.
    /// `id` ties the Start/Stop events together so parallel tool calls
    /// don't overwrite each other's args (v0.8.8 tool-identity fix).
    ToolCall {
        id: String,
        name: String,
        args: String,
        /// When true the rendered preview is collapsed to a single line and
        /// the full args are hidden until the user toggles via Ctrl+O.
        collapsed: bool,
    },
    /// Tool result. `Ok`/`Err` reflects `ToolOutput.is_error`.
    /// `id` + `name` let the renderer show `✓ bash (4ms)` instead of
    /// just `✓` — without `name` the user can't tell which of N
    /// parallel tool results this belongs to (v0.8.8 tool-identity).
    ToolResult {
        id: String,
        name: String,
        ok: bool,
        content: String,
        /// When true the body of the result is hidden — only a one-line
        /// summary is shown. Ctrl+O toggles.
        collapsed: bool,
        /// Wall-clock duration in milliseconds. Rendered as
        /// `Took N.Ns` (Pi-style) on the result line. `None` when
        /// the tool didn't measure its own duration.
        duration_ms: Option<u64>,
    },
    /// System-injected divider (turn boundary).
    Divider,
    /// v0.8.4 (ux-001): surfaced at the end of an assistant turn when
    /// the model stopped for a non-success reason (`aborted`, `length`
    /// truncation, `error`). The string is the human-readable label.
    /// Mirrors Pi's `AssistantMessageComponent` end-of-turn notice
    /// ("Response was truncated before completion." / "Operation
    /// aborted" / "Error: …").
    StopNotice(String),
    /// A system notice the TUI itself produced — not model output.
    ///
    /// The terminal guard ([`crate::terminal_guard`]) turns every line the
    /// library writes to stderr into one of these, so a diagnostic that
    /// used to land on top of the painted screen becomes a row in the
    /// transcript, where it can be read and scrolled like everything else.
    Notice(String),
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

/// v0.8.9 (perf): cheap change-detector for a transcript line.
///
/// Deliberately O(1) — it exists so the render path can ask "has this
/// line changed since I last counted its rows?" without walking the
/// content. The variant tag catches a type swap; the byte length
/// catches the common streaming case where text is appended.
///
/// A line edited to exactly the same length with different bytes would
/// collide. That cannot produce wrong *content* — only a stale row
/// count, and therefore a slightly-off scroll position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineFingerprint {
    pub tag: u8,
    pub len: usize,
}

impl TranscriptLine {
    /// O(1) fingerprint of this line's render-relevant content.
    pub fn content_fingerprint(&self) -> LineFingerprint {
        let (tag, len) = match self {
            TranscriptLine::User(s) => (0u8, s.len()),
            TranscriptLine::AssistantText(s) => (1u8, s.len()),
            TranscriptLine::ThinkingText(s) => (2u8, s.len()),
            TranscriptLine::ToolCall {
                name,
                args,
                collapsed,
                ..
            } => (3u8, name.len() + args.len() + u8::from(*collapsed) as usize),
            TranscriptLine::ToolResult {
                content,
                collapsed,
                name,
                ..
            } => (
                4u8,
                content.len() + name.len() + u8::from(*collapsed) as usize,
            ),
            TranscriptLine::Divider => (5u8, 0),
            TranscriptLine::StopNotice(s) => (6u8, s.len()),
            // Tag 8: `BashExecution` took 7. A tag collision would make
            // two different lines look identical to the row-count cache.
            TranscriptLine::Notice(s) => (8u8, s.len()),
            TranscriptLine::BashExecution {
                cmd,
                output,
                stderr,
                collapsed,
                ..
            } => (
                7u8,
                cmd.len() + output.len() + stderr.len() + u8::from(*collapsed) as usize,
            ),
        };
        LineFingerprint { tag, len }
    }

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

    // ---- v0.10.2: visual-row geometry -------------------------------
    //
    // Pi defines `cursorUp`/`cursorDown` over **visual** rows
    // (`editor.ts:833`), which are soft-wrapped rows: `buildVisualLineMap`
    // splits on `\n` *and* on the wrap width. The same is true of the
    // cursor placement in nini's own `prompt_cursor_geometry`
    // (`render.rs`: `row = explicit_rows + column_in_line / width`), so
    // a caret that only understood `\n` would disagree with the pixel it
    // is drawn on for any prompt wider than the box. That is the same
    // "implements it but computes it wrong" family as §22.4's
    // `stop_reason` casing: the feature exists, the arithmetic does not
    // match the thing it is supposed to describe.
    //
    // `width == 0` means "no frame rendered yet": rows are then `\n`
    // separated only, which is the correct fallback for a single-row
    // prompt and never worse than refusing to move.

    /// The `(byte_offset, display_width)` of every visual row.
    ///
    /// A row longer than `width` contributes `ceil(w / width)` rows; the
    /// last one holds the remainder. A zero-width row still contributes
    /// one row, because that is what renders.
    fn visual_rows(&self, width: usize) -> Vec<(usize, usize)> {
        visual_rows_of(&self.text, width)
    }

    /// Caret position as `(visual_row, display_column)`.
    pub fn visual_position(&self, width: usize) -> (usize, usize) {
        let cursor =
            crate::width::floor_char_boundary(&self.text, self.cursor.min(self.text.len()));
        let rows = self.visual_rows(width);
        for (i, &(start, w)) in rows.iter().enumerate() {
            let end = start + byte_len_for_width(&self.text, start, w);
            if cursor <= end {
                let before = &self.text[start..cursor];
                return (i, crate::width::display_width(before));
            }
        }
        let last = rows.len().saturating_sub(1);
        let start = rows[last].0;
        (last, crate::width::display_width(&self.text[start..cursor]))
    }

    /// How many visual rows the prompt occupies.
    pub fn visual_row_count(&self, width: usize) -> usize {
        self.visual_rows(width).len()
    }

    /// Byte offset for `(visual_row, display_column)`, clamping the
    /// column to the row's end — the behaviour of Pi's `moveCursor`,
    /// which places the caret at the target row and then "clamps to the
    /// line length" (`editor.ts`, `moveCursor`).
    pub fn cursor_at_visual(&self, visual_row: usize, column: usize, width: usize) -> usize {
        let rows = self.visual_rows(width);
        let row = visual_row.min(rows.len().saturating_sub(1));
        let (start, row_width) = rows[row];
        if column >= row_width {
            return start + byte_len_for_width(&self.text, start, row_width);
        }
        let mut used = 0usize;
        let mut offset = start;
        for ch in self.text[start..].chars() {
            if used >= column {
                break;
            }
            let w = crate::width::display_width(&ch.to_string());
            if used + w > column {
                // A wide glyph straddles the target column; Pi lands on
                // the glyph boundary, so stop before consuming it.
                break;
            }
            used += w;
            offset += ch.len_utf8();
        }
        offset
    }

    /// One row up / one row down, keeping the display column.
    /// Returns `true` when the caret moved.
    pub fn move_visual_row(&mut self, delta: i32, width: usize) -> bool {
        let (row, col) = self.visual_position(width);
        let rows = self.visual_rows(width);
        let target = match delta {
            d if d < 0 => match row.checked_sub(1) {
                Some(r) => r,
                None => return false,
            },
            _ => {
                if row + 1 >= rows.len() {
                    return false;
                }
                row + 1
            }
        };
        let next = self.cursor_at_visual(target, col, width);
        if next == self.cursor {
            return false;
        }
        self.cursor = next;
        true
    }

    /// Caret to the start of the current **logical** line.
    ///
    /// v0.10.2: added for Pi's `cursorUp` ladder. Distinct from
    /// `move_to_start`, which is "the whole buffer" and is what
    /// `MoveLineStart` still maps to — changing that would silently
    /// redefine `Ctrl+A` for every existing user.
    pub fn move_to_line_start(&mut self) {
        let cursor = self.cursor.min(self.text.len());
        let line_start = self.text[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
        self.cursor = line_start;
    }

    /// Caret to the end of the current **logical** line.
    pub fn move_to_line_end(&mut self) {
        let cursor = self.cursor.min(self.text.len());
        let line_end = self.text[cursor..]
            .find('\n')
            .map(|i| cursor + i)
            .unwrap_or(self.text.len());
        self.cursor = line_end;
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
                self.text
                    .replace_range((self.cursor - len)..self.cursor, "");
                self.cursor -= len;
            }
        }
        let prev = if cur == 0 {
            self.kill_ring.len() - 1
        } else {
            cur - 1
        };
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

// ---- v0.10.2: visual-row helpers --------------------------------------
//
// Shared by `InputBuffer::visual_position` / `cursor_at_visual` and by the
// `move_visual_row` callers in `runtime.rs`. All of them work in **display
// columns**, not bytes: a CJK glyph is two columns wide, so byte
// arithmetic would place the caret mid-glyph and make `Up` visibly jump
// sideways.

/// `(byte_offset, display_width)` for every visual row of `text`.
///
/// Rows break on `\n` and on the wrap `width`. `width == 0` means "unknown
/// width" (no frame has been rendered yet) and treats every logical line
/// as one row.
fn visual_rows_of(text: &str, width: usize) -> Vec<(usize, usize)> {
    let mut rows: Vec<(usize, usize)> = Vec::new();
    let mut line_start = 0usize;
    let mut i = 0usize;
    loop {
        if i == text.len() || text[i..].starts_with('\n') {
            push_line_rows(text, line_start, i, width, &mut rows);
            if i == text.len() {
                break;
            }
            line_start = i + 1;
            i += 1;
        } else {
            i += char_at(text, i).len_utf8();
        }
    }
    if rows.is_empty() {
        rows.push((0, 0));
    }
    rows
}

fn push_line_rows(
    text: &str,
    start: usize,
    end: usize,
    width: usize,
    out: &mut Vec<(usize, usize)>,
) {
    if width == 0 {
        out.push((start, crate::width::display_width(&text[start..end])));
        return;
    }
    let mut row_start = start;
    let mut row_w = 0usize;
    let mut i = start;
    while i < end {
        let ch = char_at(text, i);
        let cw = crate::width::display_width(&ch.to_string());
        // A glyph wider than the whole row still occupies it; the `row_w
        // > 0` guard is what stops that from looping forever.
        if row_w + cw > width && row_w > 0 {
            out.push((row_start, row_w));
            row_start = i;
            row_w = 0;
        }
        row_w += cw;
        i += ch.len_utf8();
    }
    out.push((row_start, row_w));
}

/// The byte offset at which `w` display columns have been consumed
/// starting from `start`, clamped to the end of the text.
fn byte_len_for_width(text: &str, start: usize, w: usize) -> usize {
    let mut used = 0usize;
    let mut i = start;
    while i < text.len() && used < w {
        let ch = char_at(text, i);
        used += crate::width::display_width(&ch.to_string());
        i += ch.len_utf8();
    }
    i - start
}

fn char_at(text: &str, i: usize) -> char {
    text[i..].chars().next().unwrap_or('\u{fffd}')
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
/// **Esc 在当前上下文里该做什么**（v0.10.8 / A3）。
///
/// Pi 不是一个 if-else 链：它在进入压缩/重试/分支摘要之前**替换掉** Esc
/// 处理器（`autoCompactionEscapeHandler` / `retryEscapeHandler`，
/// 见 `interactive-mode.ts` 的 EscapeTarget），离开那个上下文再换回来。
///
/// nini 此前是一条 if-else 链（completion → search → pending_quit → Running →
/// clear），于是「压缩期间的 Esc」与「重试期间的 Esc」没有独立的语义——
/// 它们落进 Running 那条，碰巧接近，但不是**被设计**的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EscapeTarget {
    /// 没有回合在跑：Esc 清空输入（Pi 的 Idle）。
    #[default]
    Idle,
    /// 一个回合在跑：Esc 中止这一轮（Pi 的 Agent）。
    Agent,
    /// 正在压缩上下文：Esc 中止**压缩**（Pi 的 Compaction）。
    Compacting,
    /// 正在重试一次失败的请求：Esc 中止重试（Pi 的 Retry）。
    Retrying,
    /// 正在生成分支摘要：Esc 中止它（Pi 的 BranchSummary）。
    BranchSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// User is editing the prompt; nothing is running.
    Editing,
    /// Agent is executing a turn.
    Running,
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
/// v0.8.1 — AppState split into 5 sub-structs (transcript / run /
/// model / session / ui). The public API surface is preserved via
/// forwarding methods, but field accesses are prefixed:
///
///   * `state.input`               — InputBuffer (edit buffer)
///   * `state.transcript_state`    — TranscriptState (chat history + scroll)
///   * `state.run_state`           — RunState (state machine + stats)
///   * `state.model_state`         — ModelState (LLM config)
///   * `state.session_state`       — SessionState (persistence + cwd)
///   * `state.ui_state`            — UiState (overlays + theme + debug)
///
/// Rationale: AppState grew to 38 pub fields / 70 pub methods in
/// v0.8.0-pre1. Adding any new field required touching 6+ files
/// (state.rs + main.rs + render.rs + 3+ tests). Splitting by
/// concern reduces future surface area for additive changes.
#[derive(Debug)]
pub struct AppState {
    pub input: InputBuffer,
    /// v0.8.9 (perf): render-side cache of per-line row counts.
    ///
    /// Lives on `AppState` rather than on `TranscriptState` because the
    /// render path takes `&AppState` and must not mutate transcript
    /// state (that would need `Arc::make_mut`, i.e. a clone). `Mutex`
    /// because `AppState` is shared across the render loop and the
    /// agent task, and this must stay `Send`.
    ///
    /// Invalidate with [`AppState::invalidate_item_counts`] whenever
    /// line *indices* shift.
    pub item_cache: Arc<StdMutex<TranscriptItemCache>>,
    /// v0.10: how many `TranscriptLine`s one page of scrolling covers.
    ///
    /// Published by the render path, because that is the only place that
    /// knows both the per-line row counts and the viewport height — and
    /// `ScrollUp` used to guess. It guessed `1`, which meant a single
    /// PageUp moved anywhere from one to fifty rows depending on which
    /// line it landed on: a folded tool result is many rows tall, a
    /// divider is one.
    ///
    /// Atomic rather than a plain field for the same reason as
    /// `item_cache`: `render_frame` takes `&AppState` and must not mutate.
    /// The value is recomputed every frame, so a stale read costs one
    /// keypress and self-corrects on the next frame.
    pub page_lines: Arc<AtomicUsize>,
    /// v0.10.2: the inner width of the prompt box, in cells.
    ///
    /// Published by the render path for the same reason as `page_lines`:
    /// it is the only place the width exists. Vertical caret motion is
    /// defined over *visual* rows — soft-wrapped, not just `\n`-separated
    /// — so the key handler needs it. Pi has the identical dependency and
    /// the identical answer: `buildVisualLineMap(this.lastWidth)` with
    /// `lastWidth` recorded during render (`editor.ts:416`).
    ///
    /// 0 means "no frame has been rendered yet", which the movement code
    /// reads as "do not soft-wrap", i.e. treat every `\n` as one row.
    pub prompt_inner_width: Arc<AtomicUsize>,
    /// v0.10: per-line rendered row counts, as measured by the renderer.
    ///
    /// The render path is the only place that knows how tall each
    /// `TranscriptLine` is, and the mouse needs that to convert a scrollbar
    /// drag into a `scroll_offset`. Without it the only honest conversion is
    /// "assume one row per line", which is the same mistake PageUp used to
    /// make.
    ///
    /// Published wholesale rather than read back out of `item_cache`,
    /// because the cache is keyed by width and a miss would leave the
    /// conversion with nothing to work from.
    pub line_heights: Arc<StdMutex<Vec<usize>>>,
    /// v0.10: where the transcript was last drawn. A mouse event carries
    /// only a screen position, and turning that into a scroll position
    /// requires knowing which rectangle it landed in.
    pub transcript_viewport: Arc<StdMutex<crate::scrollbar::Viewport>>,
    /// Why the real provider could not be started, if it could not.
    ///
    /// The TUI falls back to the scripted fixture provider so that it
    /// can start at all — but a silent fallback is a lie: the status
    /// bar would name a backend that is not there, and the user's
    /// message would go in and nothing would come back.
    ///
    /// The reason lives in the state (not in a one-off notice) because
    /// the state outlives the notice: the moment the user changes the
    /// model with `/model`, a notice-only warning scrolls away and the
    /// status bar goes back to looking normal. With the reason in the
    /// state, every later model change says it again.
    pub provider_fallback_reason: Option<String>,
    /// v0.10.x: how many **rendered rows** the viewport sits above the
    /// live tail — the scrollbar's unit.
    ///
    /// Published by the render path for the same reason as `line_heights`:
    /// the renderer is the only place that knows how tall each
    /// `TranscriptLine` is, and the scrollbar's geometry is expressed in
    /// rows, not in lines.
    ///
    /// The mouse used to hit-test the scrollbar with
    /// `transcript_state.scroll_offset`, which counts `TranscriptLine`s —
    /// a *different unit* from the one the renderer drew the thumb with.
    /// With one-row lines the two coincide and the bug is invisible; with
    /// a wrapped assistant message or a tall tool result they do not, and
    /// a press on the visible thumb landed somewhere else entirely, so the
    /// thumb jumped.
    ///
    /// The mouse now uses this number, so the render and the hit-test
    /// always speak the same unit.
    pub items_skipped_from_bottom: Arc<AtomicUsize>,
    /// v0.10: milliseconds since the epoch of the last scroll action, used
    /// by the scrollbar's `auto` mode to fade itself out
    /// (`scroll-view.ts` `transientScrollbarVisible` + a 1000 ms timer).
    /// nini compares timestamps rather than owning a timer, which gives the
    /// same result with no background task. Atomic for the same reason as
    /// `page_lines`: the render path takes `&AppState`.
    pub scroll_activity: Arc<AtomicU64>,
    /// v0.8.5: `Arc<TranscriptState>` so that `AppState::clone()` is
    /// O(1) instead of deep-cloning every TranscriptLine (each
    /// ToolResult can carry ~25 KB of file content, so 14 parallel
    /// reads == ~350 KB memcpy per render-frame). The `Arc` is
    /// write-through-coW via `Arc::make_mut` in mutation paths;
    /// since mutation paths hold `&mut AppState` via the outer
    /// `Mutex`, `make_mut` always returns the unique inner buffer
    /// (no clone-on-write triggered).
    pub transcript_state: Arc<TranscriptState>,
    pub run_state: RunState,
    pub model_state: ModelState,
    pub session_state: SessionState,
    pub ui_state: UiState,
}

// =================================================================
// TranscriptState — chat history + scroll state.
// =================================================================

/// v0.8.9 (perf): cache of "how many screen rows does transcript line
/// `i` expand to", for a given terminal width.
///
/// `render_transcript` needs this to place the autoscroll window and
/// the `↑ N more above` indicator. It used to get it by rendering
/// **every** line in the transcript, in full, on **every** frame —
/// markdown parsing, per-character cell-width truncation, the lot.
///
/// Measured, release build: a linear **0.545 ms/KB** of transcript with
/// essentially no intercept:
///
/// ```text
///   16 lines /   8 KB →   5.9 ms/frame
///  307 lines / 164 KB →  90.7 ms/frame
/// 1227 lines / 661 KB → 361.5 ms/frame
/// ```
///
/// A 16 ms budget is 60 fps, so ~29 KB saturates a frame and anything a
/// real task accumulates makes typing in the prompt laggy. The cost is
/// also **independent of how much is on screen**: 34 visible rows,
/// 1227 lines rendered, the other 1193 discarded immediately.
///
/// Keyed on `(width, content fingerprint)`, which makes it
/// self-invalidating — a streamed `AssistantText` grows, its length
/// changes, its entry misses, and only that one line is re-counted. The
/// count depends on content and width and on nothing else (the theme
/// only affects styling), so no theme dimension is needed.
///
/// A stale hit can produce a slightly-off *scroll position*, never wrong
/// content, and only if a line changed to exactly the same byte length.
#[derive(Debug, Default)]
pub struct TranscriptItemCache {
    entries: Vec<Option<ItemCountCacheEntry>>,
}

#[derive(Debug, Clone, Copy)]
struct ItemCountCacheEntry {
    width: u16,
    fingerprint: LineFingerprint,
    count: usize,
}

impl TranscriptItemCache {
    /// Cached row count for line `idx`, or `None` on a miss (new line,
    /// different width, or content changed since it was counted).
    pub(crate) fn get(&self, idx: usize, width: u16) -> Option<(LineFingerprint, usize)> {
        let e = self.entries.get(idx).copied().flatten()?;
        if e.width != width {
            return None;
        }
        Some((e.fingerprint, e.count))
    }

    pub(crate) fn put(
        &mut self,
        idx: usize,
        width: u16,
        fingerprint: LineFingerprint,
        count: usize,
    ) {
        if idx >= self.entries.len() {
            // Keep the vector at least as long as the live transcript.
            // Growing is cheap; we never shrink on the hot path.
            return;
        }
        self.entries[idx] = Some(ItemCountCacheEntry {
            width,
            fingerprint,
            count,
        });
    }

    /// Grow to `lines` entries, preserving what is still valid.
    pub(crate) fn ensure_len(&mut self, lines: usize) {
        if self.entries.len() < lines {
            self.entries.resize(lines, None);
        }
    }

    /// Drop everything. Used when line *indices* shift (compaction
    /// folding, `/clear`, session reload), so a stored count cannot end
    /// up describing a different line.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Transcript (chat history) + display state. Owns the Vec of
/// TranscriptLine that the renderer walks, plus autoscroll +
/// scroll_offset for the viewport.
#[derive(Debug)]
pub struct TranscriptState {
    // v0.8.9 (P1-5): `scroll_offset` is an index counted from the
    // bottom of `lines`, so any operation that *shrinks* `lines`
    // invalidates it. Leaving it behind is not cosmetic — the
    // renderer's user-scroll branch skips items from the bottom until
    // the row budget runs out, and with a stale offset the budget
    // never runs out, so the transcript renders completely blank.
    // `reset_view` is the single place that snaps the viewport back to
    // the tail; every line-shrinking operation must call it.
    // `render_transcript` carries a matching defensive clamp so a missed
    // call degrades to "shows the top" instead of "shows nothing".
    /// Chat history — Vec of TranscriptLine.
    pub lines: Vec<TranscriptLine>,
    /// Auto-scroll transcript to the bottom on new lines.
    pub autoscroll: bool,
    /// Number of lines scrolled up from the bottom (0 = at bottom).
    /// Increases when user presses PageUp; decreases on PageDown;
    /// resets to 0 when user is back at bottom and `autoscroll` is true.
    pub scroll_offset: usize,
}

// =================================================================
// RunState — agent loop state machine + token statistics.
// =================================================================

/// Agent run state. Owns mode (Editing/Running/Aborted/Quitting),
/// status string, abort signaling, compaction flags, and token stats.
#[derive(Debug)]
pub struct RunState {
    /// Agent mode. Drives the input box's enabled state.
    pub mode: RunMode,
    /// **Is an agent task still running?** — a fact about the *agent*,
    /// deliberately NOT the same fact as `mode`.
    ///
    /// `mode` answers "may the editor take a submission?": Esc sets it back
    /// to `Editing` the instant it is pressed (v0.10.1/M1), so the user can
    /// type immediately after an interrupt. `agent_alive` answers "has the
    /// spawned task finished pushing events?", and stays `true` for as long
    /// as it takes — abort is a flag the task observes on its own schedule,
    /// typically a beat after the keystroke.
    ///
    /// Do not merge the two. Before this field existed, `mode == Running`
    /// was the only guard against a second `AgentDriver` invocation, and
    /// Esc removed the guard while the first agent was still streaming:
    /// Esc → Enter spawned driver #2 onto one `SharedState` and one session
    /// file (measured: `spawns=2 live=2`, see
    /// `tests/esc_concurrency_probe.rs`). This field is Pi's `_isAgentRun-
    /// Active`, set false only by the run's terminal event.
    ///
    /// Every terminal path MUST clear it — `Done`, and the watchdog in
    /// `runtime.rs` for a driver that returns without pushing `Done`. A
    /// missed reset is the worse failure of the two: it silences every
    /// later submission for the life of the process.
    pub agent_alive: bool,
    /// Esc 在当前上下文里该做什么（v0.10.8 / A3）。
    pub escape_target: EscapeTarget,
    /// **有一条消息在排队，而回合已经结束了**（v0.10.8 / A6）。
    ///
    /// 排队消息的正常归宿有两个：回合还活着时，agent 循环每轮会
    /// `get_steering_messages` 把它捞进**当前** turn（Pi 的 steer）；
    /// 回合已经收尾时，就没人再捞了——消息会一直躺在队列里，**用户说的
    /// 那句话凭空消失**。
    ///
    /// 这个标记就是「该投递了」的信号：运行循环看到它就把队列里的消息
    /// 作为**新的一轮**送出去（Pi 在 session 层做的事，nini 的等价物是
    /// 这个运行循环）。
    ///
    /// v0.11 (A4)：**任一**队列（steer 或 followUp）非空都置起它。
    pub auto_turn_pending: bool,
    /// 自动投递的次数上限（防止「投递 → 用户又排队 → 再投递」无限循环）。
    pub auto_turn_count: usize,
    /// Monotonic id of the currently-spawned agent turn. Bumped at spawn
    /// time; the watchdog that reaps a driver which never pushed `Done`
    /// only clears `agent_alive` if this still matches, so a slow reap of
    /// turn *N* can never disarm the guard on turn *N+1*.
    pub run_seq: u64,
    /// User-visible status (e.g., "running...", "ready").
    pub status: String,
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
    /// Abort signal: `trigger()` aborts the currently running agent.
    ///
    /// v0.11 (N5)：此前它是一个 `Arc<Notify>`，**没有任何消费者**——
    /// Esc 调了 `notify_waiters()`，而驱动那一侧根本没人读它，于是
    /// 「aborted」只是一个状态字符串，模型继续往下写。
    /// 而且 `Notify::notify_waiters()` **不留存许可**：先触发后等待的
    /// 那一侧会永远等不到。
    ///
    /// 现在它是 [`nini_core::abort_signal::AbortSignal`]：标志先落，通知后发；
    /// 等待方先注册再看标志。两个顺序都不会丢。
    ///
    /// 由 [`crate::runtime::run`] 在**每一轮开始时**创建（单一位置），
    /// 轮结束时清掉。
    pub abort_signal: Option<Arc<nini_core::abort_signal::AbortSignal>>,
    /// 压缩专用的中止信号（Pi 的 `_autoCompactionAbortController`）。
    ///
    /// v0.11 (A2b)：压缩期间的 Esc **只**取消压缩，不中止这一轮——
    /// 这是 Pi 的 `autoCompactionEscapeHandler` 做的事。
    /// 与 `abort_signal` 分开，是因为两者的后果不同：
    /// 一个终止整个回合，一个只终止压缩。
    pub abort_compaction_signal: Option<Arc<nini_core::abort_signal::AbortSignal>>,
    /// v0.11 (B5)：上一次「计入 double-Escape」的 Esc 的时间戳（毫秒）。
    ///
    /// Pi: `interactive-mode.ts#lastEscapeTime`。**只在编辑器为空时**
    /// 记录（Pi 的规则），窗口是 500 ms 的**挂钟**窗口。
    pub last_escape_ms: Option<u64>,
    /// True while a manual compaction is in progress. When true,
    /// /model /model/<arg> still works but the next prompt gets queued.
    pub is_compacting: bool,
    /// True while auto-compaction is in progress. When true,
    /// /model /model/<arg> still works but the next prompt gets queued.
    pub is_auto_compacting: bool,
    /// **steer 队列**：回合在跑时提交、要插进**当前这一轮**的消息
    /// （Pi 的 steering）。
    ///
    /// Pi: `agent-session.ts#_steeringMessages`。流式中的 Enter 走这里
    /// （`interactive-mode.ts#setupEditorSubmitHandler` →
    /// `prompt(text,{streamingBehavior:"steer"})`）；压缩期间的 Enter
    /// 也走这里（`interactive-mode.ts#queueCompactionMessage` 的
    /// `"steer"` 分支）。
    ///
    /// **只有用户说的话**进这个队列。工具输出、bash 结果、分支摘要
    /// 都不进（Pi 把 bash 结果记进 session 的活消息表，不当成用户指令）。
    pub pending_steer_messages: Vec<String>,
    /// **followUp 队列**：回合在跑时用 Alt+Enter 提交、要等**这一轮结束
    /// 之后**再执行的消息（Pi 的 followUp）。
    ///
    /// Pi: `agent-session.ts#_followUpMessages`。Alt+Enter 走这里
    /// （`interactive-mode.ts#handleFollowUp` → `prompt(text,
    /// {streamingBehavior:"followUp"})`）；压缩期间的 Alt+Enter 也走这里
    /// （`interactive-mode.ts#queueCompactionMessage` 的 `"followUp"`
    /// 分支）。
    ///
    /// **只有用户说的话**进这个队列（同上）。
    pub pending_followup_messages: Vec<String>,
    /// Provider's context-window size (tokens). Used to compute
    /// `context_percent` shown in the status bar. Mirrors Pi's
    /// `getContextUsage()` percent.
    pub context_window: u32,
    /// Last API-reported `usage.input` token count. Combined with
    /// `cache_read_tokens` and `context_window` to display a progress
    /// bar in the status bar.
    pub context_used: u32,
    /// Total estimated cost (USD) for the session, surfaced in the
    /// status bar when non-zero. Mirrors Pi's footer.
    pub cost_usd: f64,
    /// Cumulative token usage for the session.
    pub tokens: TokenStats,
}

// =================================================================
// ModelState — LLM configuration.
// =================================================================

/// LLM configuration: which model, which provider, which thinking
/// level, what cycling models are available, where settings live.
#[derive(Debug)]
pub struct ModelState {
    /// Current model ID (e.g., "MiniMax-M3").
    pub model: String,
    /// Provider name (e.g. "anthropic", "openai", "minimax"). Set
    /// at bootstrap from the `--provider` flag or `NINI_PROVIDER` env.
    /// Rendered in the status bar (Pi-style `(provider) model`)
    /// so users can tell at a glance which backend they're on.
    pub provider: Option<String>,
    /// Current thinking level (e.g. "off", "medium", "high"). Updated
    /// by cycle_thinking and the /thinking command. Used as a
    /// cycle_thinking source of truth so we don't have to parse it
    /// out of the status string.
    pub thinking_level: Option<String>,
    /// Models available for cycling with Ctrl+P (loaded from
    /// `~/.nini/agent/models.json` or `models.json`). Empty means
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
    /// Settings are loaded from `~/.nini/agent/settings.json` if present;
    /// otherwise we use `Settings::default()`.
    pub settings_snapshot: crate::settings::Settings,
    /// Models the user's `models.json` declares that the shipped catalog
    /// does not have.
    ///
    /// They are a **fact about the user's setup** — a model reached through
    /// their own gateway, with the context window they wrote down — so
    /// nini uses their window for compaction instead of the 200 000
    /// fallback, and offers them in the `/model` selector.
    ///
    /// Empty by default: the field is filled by the CLI at startup from
    /// `models.json`, never read from the environment behind the app's
    /// back. A test that constructs an `AppState` therefore sees exactly
    /// what it declared.
    pub user_models: Vec<nini_core::model_runtime::ModelSpec>,
}

// =================================================================
// SessionState — persistence + working-directory metadata.
// =================================================================

/// Take the session lock, waiting for it.
///
/// v0.10 (fix): the eleven call sites this replaces all did
/// `arc.try_lock()` and each one had invented its own answer to "the lock
/// is busy":
///
/// | caller | what a busy lock meant |
/// |---|---|
/// | `session_context_messages` | an empty context — the model loses the conversation |
/// | `session_append_message` | the turn is never recorded |
/// | `session_compact` | compaction silently does not happen |
/// | `transcript_from_context` | the transcript stops matching the session |
/// | `/tree`, branch summary | an empty tree, or no summary |
/// | `/fork` ×2 | "the session is busy — try again in a moment" |
///
/// None of those is a state the product has. Contention is caused by the
/// background flush thread, which holds the lock for the length of one
/// `write_to_file` — a full serialise plus an atomic rename. Every one of
/// those callers is a short synchronous section, so the right answer is to
/// wait; the flush thread no longer holds the lock across the IO either
/// (see `session_flush`).
///
/// Poisoning is recovered the same way `lock_state` recovers it: a panic
/// while holding the session lock leaves the session intact, and refusing to
/// ever read it again would turn a recoverable panic into total data loss.
pub fn lock_session(
    session: &Arc<StdMutex<nini_session::Session>>,
) -> std::sync::MutexGuard<'_, nini_session::Session> {
    session.lock().unwrap_or_else(|e| e.into_inner())
}

/// Active session + cwd/git-branch metadata shown in the status bar.
#[derive(Debug)]
pub struct SessionState {
    pub session_id: Option<String>,
    /// Active session for persistence. When None, no session is active
    /// (e.g., --no-session mode). Arc+Mutex allows AgentSink (tokio task)
    /// to flush entries while the render loop holds a read handle.
    ///
    /// v0.10 (fix): `StdMutex`, not `tokio::sync::Mutex` — see the module
    /// preamble. No session guard is ever held across an `.await`, so the
    /// async mutex bought nothing and its `try_lock` bought eleven silent
    /// failure modes.
    pub session: Option<Arc<StdMutex<nini_session::Session>>>,
    /// Path to the session file on disk. Used for atomic write.
    pub session_path: Option<PathBuf>,
    /// Current working directory for the status bar (e.g., "~/nini").
    /// `None` until the runtime populates it.
    pub cwd: Option<PathBuf>,
    /// Current git branch name, if any. `None` outside a git repo or
    /// before the runtime populates it. Mirrors Pi's footer.
    pub git_branch: Option<String>,
    /// v0.8.9 (P2-9): how many lines the last session load had to skip.
    ///
    /// The reader is now tolerant of malformed lines, which is the right
    /// behaviour — but tolerance that is invisible is its own kind of
    /// lie: the user would see a history with a hole in it and no
    /// explanation. The runtime surfaces this as a `StopNotice` so the
    /// gap is on screen next to the content it affects.
    pub skipped_lines_on_load: usize,
    /// The assistant turn currently being assembled for persistence.
    ///
    /// P0.1: before this, the agent's side of a conversation never reached
    /// the session file. A complete turn left exactly one entry — the user
    /// message. Assistant prose arrived as `TextDelta` -> `push_assistant_raw`
    /// (transcript only), and `push_tool_call` / `push_tool_result` never
    /// touched the session at all. `state.rs` carried the comment "Tool
    /// results are part of the turn — session_append is handled on TurnEnd";
    /// nothing handled them.
    ///
    /// Pi's shape is one assistant message holding the text plus every
    /// `toolCall` block, followed by one message per tool result. The event
    /// stream gives us those pieces separately and incrementally, so they
    /// are buffered here and written once the turn's shape is known.
    pub pending_assistant: Option<PendingAssistant>,
}

/// One `toolCall` block being assembled for the pending assistant message.
#[derive(Debug, Clone)]
pub struct PendingToolCall {
    pub id: String,
    pub name: String,
    /// Filled in by `ToolCallStop`. The provider streams the arguments
    /// after announcing the call, so at `ToolCallStart` they are not known.
    pub args: Option<String>,
}

/// An assistant turn under construction: prose plus the calls it made.
#[derive(Debug, Clone, Default)]
pub struct PendingAssistant {
    pub text: String,
    pub tool_calls: Vec<PendingToolCall>,
}

// =================================================================
// UiState — overlays, theme, debug.
// =================================================================

/// UI-specific state: floating panels, theme, edit-diff indicator,
/// debug logging toggle. Mostly read by the renderer.
#[derive(Debug)]
pub struct UiState {
    /// Currently-loaded theme name. Mirrors settings.theme but kept
    /// here so the status bar can render the name without holding
    /// the settings lock. `None` means default (dark) theme.
    pub theme_name: Option<String>,
    /// Last edit-tool diff: (additions, deletions) in lines. Surfaced
    /// briefly in the status bar after each edit. `None` when no
    /// edit has been run yet, or when the user explicitly clears
    /// the indicator. Mirrors Pi's `[edit +N -M]` status pill.
    pub last_diff: Option<(usize, usize)>,
    /// Whether the F1 key was toggled on. The renderer swaps the bottom
    /// footer between a short hint set and an extended hint set so the
    /// user can see ALL key bindings without scrolling.
    pub help_extended: bool,
    /// `settings.fullscreenScrollbar`: when the transcript scrollbar is
    /// painted. Mirrors Pi's `ScrollViewScrollbar`, default `auto`.
    /// Held here rather than read from the settings manager on every frame,
    /// for the same reason `theme_name` is.
    pub fullscreen_scrollbar: crate::scrollbar::ScrollbarMode,
    /// v0.10: a scrollbar drag in progress, or `None`. Set on a left press
    /// over the bar's column, cleared on release. `None` therefore means
    /// "not dragging", which is what every mouse event checks first.
    pub scrollbar_drag: Option<crate::runtime::ScrollbarDrag>,
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
    /// Optional completion popup (slash commands; extensible to `@file`).
    pub completion: Option<CompletionPopup>,
    /// Whether verbose debug logging is on (toggled by `/debug`). When
    /// true, the runtime appends per-keystroke lines to
    /// `~/.nini/state.log` so users can `tail -f` it.
    pub debug_logging: bool,
}

// =================================================================
// Clone / Default impls (hand-written due to Box<dyn SelectorState>)
// =================================================================

// v0.8.1 review fix: `#[derive(Clone)]` on a struct containing
// `Option<Box<dyn SelectorState>>` fails to compile (verified with
// rustc: E0277 "the trait bound `dyn SelectorState: Clone` is not
// satisfied"). So all 5 sub-structs + AppState need manual Clone
// impls. UiState's selector is reset to None, mirroring the v0.8.0
// behavior of clone_for_render.

impl TranscriptState {
    /// v0.8.9 (P1-5): put the viewport back at the tail.
    ///
    /// Call this after **any** operation that removes lines from the
    /// front or middle (compaction folding, `/clear`, session reload).
    /// A `scroll_offset` carried over from a longer transcript points
    /// past the new end, and the transcript area renders empty with
    /// only a misleading `↓ N more` in the corner.
    pub fn reset_view(&mut self) {
        self.autoscroll = true;
        self.scroll_offset = 0;
    }

    /// Clamp `scroll_offset` to something the current line count can
    /// actually satisfy. Returns `true` if it had to be corrected.
    ///
    /// The renderer's user-scroll path translates `scroll_offset`
    /// (lines) into an item-space skip. If that skip exceeds the total
    /// item count, the row budget is never consumed and the transcript
    /// comes out blank. Clamping here means the worst case of a missed
    /// `reset_view` is "the user sees the top of the transcript"
    /// instead of "the user sees an empty box".
    pub fn clamp_view(&mut self) -> bool {
        let max = self.lines.len().saturating_sub(1);
        if self.scroll_offset > max {
            self.scroll_offset = max;
            true
        } else {
            false
        }
    }
}

impl Clone for TranscriptState {
    fn clone(&self) -> Self {
        Self {
            lines: self.lines.clone(),
            autoscroll: self.autoscroll,
            scroll_offset: self.scroll_offset,
        }
    }
}

impl Default for TranscriptState {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            autoscroll: true,
            scroll_offset: 0,
        }
    }
}

impl Clone for RunState {
    fn clone(&self) -> Self {
        Self {
            mode: self.mode,
            agent_alive: self.agent_alive,
            escape_target: self.escape_target,
            auto_turn_pending: self.auto_turn_pending,
            auto_turn_count: self.auto_turn_count,
            run_seq: self.run_seq,
            status: self.status.clone(),
            pending_quit: self.pending_quit,
            pending_external_editor: self.pending_external_editor,
            abort_signal: self.abort_signal.clone(),
            abort_compaction_signal: self.abort_compaction_signal.clone(),
            last_escape_ms: self.last_escape_ms,
            is_compacting: self.is_compacting,
            is_auto_compacting: self.is_auto_compacting,
            pending_steer_messages: self.pending_steer_messages.clone(),
            pending_followup_messages: self.pending_followup_messages.clone(),
            context_window: self.context_window,
            context_used: self.context_used,
            cost_usd: self.cost_usd,
            tokens: self.tokens.clone(),
        }
    }
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            mode: RunMode::Editing,
            agent_alive: false,
            escape_target: EscapeTarget::Idle,
            auto_turn_pending: false,
            auto_turn_count: 0,
            run_seq: 0,
            status: "ready".to_string(),
            pending_quit: None,
            pending_external_editor: false,
            abort_signal: None,
            abort_compaction_signal: None,
            last_escape_ms: None,
            is_compacting: false,
            is_auto_compacting: false,
            pending_steer_messages: Vec::new(),
            pending_followup_messages: Vec::new(),
            context_window: 0,
            context_used: 0,
            cost_usd: 0.0,
            tokens: TokenStats::default(),
        }
    }
}

impl Clone for ModelState {
    fn clone(&self) -> Self {
        Self {
            model: self.model.clone(),
            provider: self.provider.clone(),
            thinking_level: self.thinking_level.clone(),
            models_cycle: self.models_cycle.clone(),
            models_cycle_idx: self.models_cycle_idx,
            settings_path: self.settings_path.clone(),
            settings_snapshot: self.settings_snapshot.clone(),
            user_models: self.user_models.clone(),
        }
    }
}

impl Default for ModelState {
    fn default() -> Self {
        Self {
            model: String::new(),
            provider: None,
            thinking_level: None,
            models_cycle: Vec::new(),
            models_cycle_idx: None,
            settings_path: None,
            settings_snapshot: crate::settings::Settings::default(),
            user_models: Vec::new(),
        }
    }
}

impl Clone for SessionState {
    fn clone(&self) -> Self {
        Self {
            session_id: self.session_id.clone(),
            session: self.session.clone(),
            session_path: self.session_path.clone(),
            cwd: self.cwd.clone(),
            git_branch: self.git_branch.clone(),
            skipped_lines_on_load: self.skipped_lines_on_load,
            pending_assistant: self.pending_assistant.clone(),
        }
    }
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            session_id: None,
            session: None,
            session_path: None,
            cwd: None,
            git_branch: None,
            skipped_lines_on_load: 0,
            pending_assistant: None,
        }
    }
}

impl Clone for UiState {
    fn clone(&self) -> Self {
        Self {
            theme_name: self.theme_name.clone(),
            last_diff: self.last_diff,
            help_extended: self.help_extended,
            fullscreen_scrollbar: self.fullscreen_scrollbar,
            scrollbar_drag: self.scrollbar_drag,
            close_help: self.close_help,
            search: self.search.clone(),
            // Box<dyn> is not Clone — drop selector (mirrors v0.8.0 clone_for_render)
            selector: None,
            completion: self.completion.clone(),
            debug_logging: self.debug_logging,
        }
    }
}

impl UiState {
    /// The transcript scrollbar mode. The renderer reads this rather than
    /// the settings manager, which it does not hold.
    pub fn scrollbar_mode(&self) -> crate::scrollbar::ScrollbarMode {
        self.fullscreen_scrollbar
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            theme_name: None,
            last_diff: None,
            help_extended: false,
            fullscreen_scrollbar: crate::scrollbar::ScrollbarMode::default(),
            scrollbar_drag: None,
            close_help: false,
            search: None,
            selector: None,
            completion: None,
            debug_logging: false,
        }
    }
}

impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            input: self.input.clone(),
            // v0.8.5: Arc bump instead of deep clone. See the
            // `transcript_state` field comment for why this matters.
            transcript_state: Arc::clone(&self.transcript_state),
            // v0.8.9 (perf): share the cache on clone. A `from`-clone
            // gets a *copy* of the transcript (fork), so it must not
            // inherit counts that describe the other copy's lines —
            // see the fingerprint check, which catches a same-length
            // coincident line, and `invalidate_item_counts` for the
            // from-scratch case.
            item_cache: Arc::clone(&self.item_cache),
            page_lines: Arc::clone(&self.page_lines),
            prompt_inner_width: Arc::clone(&self.prompt_inner_width),
            line_heights: Arc::clone(&self.line_heights),
            transcript_viewport: Arc::clone(&self.transcript_viewport),
            items_skipped_from_bottom: Arc::clone(&self.items_skipped_from_bottom),
            provider_fallback_reason: self.provider_fallback_reason.clone(),
            scroll_activity: Arc::clone(&self.scroll_activity),
            run_state: self.run_state.clone(),
            model_state: self.model_state.clone(),
            session_state: self.session_state.clone(),
            ui_state: self.ui_state.clone(),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            input: InputBuffer::new(),
            transcript_state: Arc::new(TranscriptState::default()),
            item_cache: Arc::new(StdMutex::new(TranscriptItemCache::default())),
            page_lines: Arc::new(AtomicUsize::new(1)),
            prompt_inner_width: Arc::new(AtomicUsize::new(0)),
            line_heights: Arc::new(StdMutex::new(Vec::new())),
            transcript_viewport: Arc::new(StdMutex::new(crate::scrollbar::Viewport::default())),
            items_skipped_from_bottom: Arc::new(AtomicUsize::new(0)),
            provider_fallback_reason: None,
            scroll_activity: Arc::new(AtomicU64::new(0)),
            run_state: RunState::default(),
            model_state: ModelState::default(),
            session_state: SessionState::default(),
            ui_state: UiState::default(),
        }
    }
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
    pub fn set_status_bar_metadata(&mut self, cwd: Option<PathBuf>, git_branch: Option<String>) {
        self.session_state.cwd = cwd;
        self.session_state.git_branch = git_branch;
    }

    /// Update cost + context usage shown in the status bar.
    pub fn set_status_bar_usage(&mut self, cost_usd: f64, context_window: u32, context_used: u32) {
        self.run_state.cost_usd = cost_usd;
        self.run_state.context_window = context_window;
        self.run_state.context_used = context_used;
    }

    /// Cheap clone for the render loop. Skips the `selector` field
    /// (Box<dyn SelectorState> doesn't implement Clone).
    /// Cheap clone for the render loop. Drops `ui_state.selector`
    /// (Box<dyn SelectorState> doesn't implement Clone). After the
    /// v0.8.1 split, this is essentially `.clone()`, but we keep
    /// the explicit method as a documentation marker.
    pub fn clone_for_render(&self) -> Self {
        self.clone()
    }

    /// Deep clone. Excludes the `selector` field (Box<dyn> not Clone).
    /// Use `clone_for_render` for the common render path.
    pub fn clone_full(&self) -> Self {
        self.clone()
    }

    pub fn new(model: impl Into<String>) -> Self {
        let mut s = Self::default();
        s.model_state.model = model.into();
        s
    }

    /// v0.11 (B4): **bash mode** — the editor holds a shell command.
    ///
    /// Pi: `interactive-mode.ts:2907` —
    /// `this.isBashMode = text.trimStart().startsWith("!")`.
    ///
    /// The state is **derived**, not a flag: Pi keeps a boolean only
    /// because it has to know when to repaint the border, and nini derives
    /// the same answer from the same text — so there is no way for the two
    /// to disagree. (Pi's `isBashMode` is cleared explicitly on Esc and
    /// after a bash submit; in nini both of those clear the text, and
    /// empty text is not bash mode.)
    ///
    /// Note the deliberate asymmetry with the **submit** path: bash mode
    /// is detected on the *trimmed* text, but a command only *runs* when
    /// the text starts with `!` (`runtime.rs`: `text.strip_prefix('!')`).
    /// That is Pi's asymmetry too (`interactive-mode.ts:3105` uses
    /// `text.startsWith("!")` for execution), so a leading space means
    /// "this looks like a command" in the border and "this is a message"
    /// on Enter.
    pub fn is_bash_mode(&self) -> bool {
        self.input.text.trim_start().starts_with('!')
    }

    /// Construct with an explicit context-window size. The runtime
    /// passes the value loaded from `~/.nini/agent/settings.json`; tests
    /// (and the no-config path) use the default constructor above.
    pub fn with_context_window(model: String, context_window: u32) -> Self {
        Self::with_settings(model, context_window, crate::settings::Settings::default())
    }

    /// Construct with an explicit context-window size AND a snapshot of
    /// the user-editable settings. The auto-compaction trigger reads
    /// `reserve_tokens` from the snapshot.
    pub fn with_settings(
        model: String,
        context_window: u32,
        settings: crate::settings::Settings,
    ) -> Self {
        let mut s = Self::default();
        s.model_state.model = model;
        s.model_state.settings_snapshot = settings;
        s.run_state.context_window = context_window;
        s
    }

    /// Initialize a new session for this working directory. Creates the session
    /// header and stores the file path for atomic writes.
    pub fn session_init(&mut self, path: PathBuf) {
        self.session_begin();
        self.session_state.session_path = Some(path);
    }

    /// Start a conversation that has nowhere to be written.
    ///
    /// Used when nini cannot work out where sessions live - no `HOME` and
    /// no `NINI_AGENT_DIR`. The conversation still runs; it simply is not
    /// recorded, and `session_path` stays `None` instead of pointing at
    /// somewhere invented. `/new` used to fall back to `"."` here, so a
    /// session file would quietly appear inside whatever project the user
    /// happened to be standing in.
    /// **启动即建立会话**（v0.10.8 / A5）。
    ///
    /// 此前 nini 启动后不建会话文件——footer 显示 `[no session]`，对话在退出后
    /// **整个消失**，`/resume` 也列不出任何东西。Pi 每次启动一个会话就有一个文件
    /// （`~/.pi/agent/sessions/<project>/…`）。
    ///
    /// 这里只**定下路径**（状态里有 id、有去处），文件本身在第一次 flush 时才
    /// 写出——所以「启动后立刻退出」不会留下一堆空文件。
    ///
    /// 没有 `HOME` 也没有 `NINI_AGENT_DIR` 时：照旧不记录，但**要说出来**
    /// （而不是让用户以为记下来了）。
    ///
    /// 它住在库里而不是 CLI 里，是为了让**测试能测到同一份实现**——
    /// 曾经有一版测试自己重写了一遍逻辑，于是「把 CLI 里的实现删掉」那条
    /// 变异注入**没有让测试变红**：测试根本没碰被测对象。
    pub fn start_session(&mut self, cwd: &std::path::Path) {
        match nini_core::paths::new_session_path(cwd) {
            Some(path) => self.session_init(path),
            None => {
                self.session_init_ephemeral();
                self.push_notice(
                    "(not recorded — set NINI_AGENT_DIR to give nini a place to keep sessions)"
                        .to_string(),
                );
            }
        }
    }

    pub fn session_init_ephemeral(&mut self) {
        self.session_begin();
        self.session_state.session_path = None;
    }

    fn session_begin(&mut self) {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let session = nini_session::Session::new(cwd);
        // v0.10 (fix): the id in `session_state` was a second, unrelated
        // `Uuid::now_v7()`, while the session itself carries
        // `u_{v4}` in its header. So the number the status bar showed and
        // the number written into the file's header were different values
        // for the same conversation — and `/fork`, which names its parent by
        // header id (`createBranchedSession`, `session-manager.ts:1441`),
        // produced a parent reference nothing could resolve.
        //
        // `session_load` has always read the id off the header; only this
        // path invented one. One source of truth.
        self.session_state.session_id = Some(session.header.id.clone());
        self.session_state.session = Some(Arc::new(StdMutex::new(session)));
    }

    /// Append a user or assistant message to the session. Call this BEFORE
    /// pushing to the transcript so the entry and transcript stay in sync.
    pub fn session_append(
        &mut self,
        parent_id: Option<String>,
        role: nini_core::Role,
        content: String,
    ) {
        let Some(ref arc) = self.session_state.session else {
            return;
        };
        let msg = nini_core::AgentMessage {
            role,
            content: vec![nini_core::ContentBlock::Text { text: content }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        let mut guard = lock_session(arc);
        let _id = guard.push_message(parent_id, msg);
    }
    /// P0.1: buffer a streamed text delta for the pending assistant message.
    pub fn session_buffer_text(&mut self, text: &str) {
        let p = self
            .session_state
            .pending_assistant
            .get_or_insert_with(PendingAssistant::default);
        p.text.push_str(text);
    }

    /// P0.1: record a tool call announcement (name known, args not yet).
    pub fn session_buffer_tool_call_start(&mut self, id: &str, name: &str) {
        let p = self
            .session_state
            .pending_assistant
            .get_or_insert_with(PendingAssistant::default);
        if !p.tool_calls.iter().any(|c| c.id == id) {
            p.tool_calls.push(PendingToolCall {
                id: id.to_string(),
                name: name.to_string(),
                args: None,
            });
        }
    }

    /// P0.1: fill in the arguments once the provider stops streaming them.
    pub fn session_buffer_tool_call_args(&mut self, id: &str, args: &str) {
        let p = self
            .session_state
            .pending_assistant
            .get_or_insert_with(PendingAssistant::default);
        if let Some(call) = p.tool_calls.iter_mut().find(|c| c.id == id) {
            call.args = Some(args.to_string());
            return;
        }
        // A Stop with no matching Start (an out-of-order event from a
        // buggy provider). Record it rather than losing the call, matching
        // what the transcript path already does.
        p.tool_calls.push(PendingToolCall {
            id: id.to_string(),
            name: String::new(),
            args: Some(args.to_string()),
        });
    }

    /// P0.1: write the pending assistant turn as ONE assistant message.
    ///
    /// Pi's assistant message carries the prose and every `toolCall` block
    /// together; splitting them would put the arguments in a message with no
    /// call and the call in a message with no arguments.
    ///
    /// No-op when nothing is buffered, so it is safe to call from both the
    /// tool-result path and `TurnEnd`.
    pub fn session_flush_assistant(&mut self) {
        let Some(pending) = self.session_state.pending_assistant.take() else {
            return;
        };
        if pending.text.is_empty() && pending.tool_calls.is_empty() {
            return;
        }

        let mut content: Vec<nini_core::entries::ContentBlock> = Vec::new();
        if !pending.text.is_empty() {
            content.push(nini_core::entries::ContentBlock::Text { text: pending.text });
        }
        for call in &pending.tool_calls {
            let args = call.args.clone().unwrap_or_default();
            let arguments =
                serde_json::from_str(&args).unwrap_or_else(|_| serde_json::Value::String(args));
            content.push(nini_core::entries::ContentBlock::ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments,
            });
        }

        self.session_append_message(nini_core::entries::AgentMessage::Assistant(
            nini_core::entries::AssistantMessage {
                content,
                api: String::new(),
                provider: String::new(),
                model: String::new(),
                usage: Default::default(),
                stop_reason: nini_core::entries::StopReason::Stop,
                error_message: None,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        ));
    }

    /// P0.1: persist one tool result as its own message, Pi-shaped.
    ///
    /// `toolCallId` is what binds a result back to its call on resume; it
    /// must be the provider's call id, not a display index.
    pub fn session_append_tool_result(
        &mut self,
        id: &str,
        name: &str,
        ok: bool,
        content: &str,
        details: Option<serde_json::Value>,
        duration_ms: Option<u64>,
    ) {
        let details =
            details.or_else(|| duration_ms.map(|ms| serde_json::json!({ "durationMs": ms })));
        self.session_append_message(nini_core::entries::AgentMessage::ToolResult(
            nini_core::entries::ToolResultMessage {
                tool_call_id: id.to_string(),
                tool_name: name.to_string(),
                content: vec![nini_core::entries::ContentBlock::Text {
                    text: content.to_string(),
                }],
                details,
                usage: None,
                is_error: !ok,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        ));
    }

    /// Record a turn that failed, as an assistant message carrying the error.
    ///
    /// v0.10 (R2h): `AgentEvent::TurnEnd` has no error payload and `run()`
    /// returns `Err` outright on a non-retryable provider error, so a failed
    /// turn left nothing in the session - the `[error]` line the user saw
    /// came from `push_assistant_raw`, which is transcript-only. Reopening
    /// the session showed a conversation quietly missing its last turn.
    ///
    /// Pi's schema has a place for exactly this: `stopReason: "error"` with
    /// `errorMessage` set (`pi-protocol/src/schemas.ts:148-154`), rendered
    /// at `assistant-message.ts:190-193`. R1f added both fields to nini so
    /// real Pi sessions would parse; this is the producer that was missing.
    pub fn session_append_turn_error(&mut self, message: &str) {
        self.session_append_message(nini_core::entries::AgentMessage::Assistant(
            nini_core::entries::AssistantMessage {
                content: vec![],
                api: String::new(),
                provider: String::new(),
                model: String::new(),
                usage: Default::default(),
                stop_reason: nini_core::entries::StopReason::Error,
                error_message: Some(message.to_string()),
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        ));
    }

    /// Record a `!cmd` execution in the session.
    ///
    /// v0.10 (R2f): `!cmd` pushed a `TranscriptLine::BashExecution` and
    /// nothing else. `BashExecutionMessage` had no production constructor
    /// anywhere in the tree, so a shell command the user ran was visible
    /// while the session was open and gone the moment they resumed - and
    /// `render_context_entries`, which rebuilds the transcript from the
    /// session, dropped the variant anyway.
    ///
    /// The command output is still injected as a user message for the model,
    /// so what the agent sees was never the problem; what a resumed session
    /// shows was.
    ///
    /// `full_output_path` stays `None`. Pi spills oversized output to a file
    /// and points at it (`bash-execution.ts:197`); nini's truncation happens
    /// in the render layer and the full text is still in memory, so there is
    /// nothing to spill *to* yet. Carrying a path nobody writes would be the
    /// same class of dead field this sweep has been removing.
    pub fn session_append_bash_execution(
        &mut self,
        cmd: &str,
        output: &str,
        stderr: &str,
        ok: bool,
        exit_code: Option<i32>,
        duration_ms: u64,
    ) {
        self.session_append_message(nini_core::entries::AgentMessage::BashExecution(
            nini_core::entries::BashExecutionMessage {
                command: cmd.to_string(),
                output: output.to_string(),
                exit_code,
                cancelled: false,
                truncated: false,
                full_output_path: None,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        ));
        let _ = (stderr, ok, duration_ms);
    }

    /// Append a pre-built session message, minting an id and a timestamp.
    fn session_append_message(&mut self, message: nini_core::entries::AgentMessage) {
        let Some(ref arc) = self.session_state.session else {
            return;
        };
        // v0.10 (fix): was `if let Ok(mut guard) = arc.try_lock()`, which
        // dropped the whole agent turn on contention — no error, no log, and
        // nothing in the file to show for it. The user asks where their tool
        // output went; the answer was a `try_lock` that did not get it.
        let mut guard = lock_session(arc);
        let id = format!("msg_{}", uuid::Uuid::new_v4());
        let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        // P0.2: chain at the leaf, like every other write. This path
        // carries the whole agent turn, so leaving it detached is what
        // made tool traffic invisible to the tree.
        guard.append_chained(nini_core::SessionEntry::Message(
            nini_core::entries::SessionMessageEntry {
                id,
                parent_id: None,
                timestamp,
                message,
            },
        ));
    }

    /// Flush pending entries to disk (atomic: temp file + rename).
    /// Logs errors but does not propagate — IO failures must not break the TUI.
    pub fn session_flush(&self) {
        let (path, session) = match (
            &self.session_state.session_path,
            &self.session_state.session,
        ) {
            (Some(p), Some(s)) => (p.clone(), s.clone()),
            _ => return,
        };
        // v0.10 (fix): the lock used to be held across `write_to_file` — a
        // full serialise of every entry plus a temp-file rename, which is
        // the entire cost of a session write. Every reader in the product
        // shares that lock: the seed built on each user turn, `/tree`, the
        // transcript rebuild, `/fork`. Making them wait would have traded
        // silent loss for a visible stall, so the IO had to leave the lock.
        //
        // `Session` is `Clone`, and the snapshot is taken inside the same
        // short critical section appends take, so what lands on disk is the
        // session as of the flush call — what the previous code wrote, minus
        // the window in which every reader was blind.
        std::thread::spawn(move || {
            let snapshot = lock_session(&session).clone();
            if let Err(e) = snapshot.write_to_file(&path) {
                // A dropped session write is invisible: the transcript shows
                // the turn and a restart does not. v0.10 wired `tracing` for
                // exactly this; `eprintln!` reached nobody unless a terminal
                // happened to be attached to the thread that lost the race.
                tracing::error!(error = %e, path = %path.display(), "session write failed");
            }
        });
    }

    /// Load a session from disk, replace the current session, and rebuild
    /// the transcript from it.
    ///
    /// v0.10 (R2a/A4): this used to load the session and nothing else.
    /// Its comment claimed "the caller rebuilds the transcript from
    /// `session.entries` right after this" - twice - and no caller did.
    /// Both call sites (`dispatch.rs` `/resume`, `runtime.rs` the session
    /// picker) stopped at the `?`.
    ///
    /// The consequence was that `/resume` swapped the history the *model*
    /// would see - A3 seeds the next Agent from the session - while the
    /// transcript the *user* looks at kept whatever it had, usually the
    /// previous conversation or nothing at all. The two disagreed about
    /// what the session contained.
    ///
    /// `build_context_entries` is the right source, not `session.entries`:
    /// it is what the agent will actually be given, so a compaction shows
    /// up as a summary rather than as the history it replaced.
    pub fn session_load(&mut self, path: PathBuf) -> Result<(), nini_session::SessionError> {
        let session = nini_session::Session::read_from_file(&path)?;
        // v0.8.9 (P2-9): record the damage. This is the only place that
        // knows what was lost on the way in.
        self.session_state.skipped_lines_on_load = session.skipped_lines;
        self.session_state.session_id = Some(session.header.id.clone());
        self.session_state.session = Some(Arc::new(StdMutex::new(session)));
        self.session_state.session_path = Some(path);
        self.transcript_from_context();
        Ok(())
    }

    /// v0.8.9 (perf): drop the per-line row-count cache.
    ///
    /// Call this whenever transcript line **indices** shift — compaction
    /// folding, `/clear`, `/new`, session reload — because a stored
    /// count would then describe whichever line moved into that slot.
    /// Purely appends (the normal case) need no call: the cache is
    /// self-invalidating on content fingerprint.
    pub fn invalidate_item_counts(&self) {
        if let Ok(mut c) = self.item_cache.lock() {
            c.clear();
        }
    }

    pub fn push_user(&mut self, text: impl Into<String>) {
        // The previous assistant turn has to land before the new user
        // message, or the session reads back out of order.
        self.session_flush_assistant();
        let s = text.into();
        Arc::make_mut(&mut self.transcript_state)
            .lines
            .push(TranscriptLine::User(s.clone()));
        self.session_append(None, nini_core::Role::User, s);
    }

    /// v0.10.8 (P2/A1): a **queued** user message — one submitted while a
    /// turn is still running.
    ///
    /// It goes into the transcript (the user must see what they queued)
    /// but **not into the session**: the session write happens when the
    /// message is actually delivered, with the next turn. Writing it here
    /// as well put the same user message into the session **twice** — once
    /// at queue time and once when the queue was flushed — so the model
    /// saw it twice and `/resume` showed it twice
    /// (review finding, 2026-10-23).
    pub fn push_user_queued(&mut self, text: impl Into<String>) {
        let s = text.into();
        Arc::make_mut(&mut self.transcript_state)
            .lines
            .push(TranscriptLine::User(s));
    }

    /// v0.10.8 (P2/A1): the **delivery** of a queued message — the session
    /// write, without a second transcript line (the transcript already
    /// showed it when it was queued).
    pub fn deliver_queued_user(&mut self, text: impl Into<String>) {
        self.session_flush_assistant();
        self.session_append(None, nini_core::Role::User, text.into());
    }

    pub fn push_assistant(&mut self, text: impl Into<String>) {
        let s = text.into();
        self.push_assistant_raw(s.clone());
        self.session_append(None, nini_core::Role::Assistant, s);
    }

    /// Internal: push to transcript without session logging. Used by AgentSink
    /// (which handles its own session flush on TurnEnd).
    ///
    /// v0.8.4 (bugfix, ux-001): streaming sends one TextDelta per
    /// token, which previously pushed one TranscriptLine::AssistantText
    /// per delta. The TUI list widget then showed N tiny rows
    /// (`Hi` / `, ` / `world`) instead of the full reply, because
    /// most lines fell outside the viewport. Now we APPEND to the
    /// last AssistantText line if there is one, but **only when it
    /// belongs to the same turn**. The previous code merged a fresh
    /// command's output (e.g. `/fork`) into the previous turn's
    /// text because it only checked the last line, not whether that
    /// last line followed a divider / tool call boundary.
    ///
    /// The boundary rule: we may append into the last AssistantText
    /// only if the line *before* it is also a ThinkingText or
    /// AssistantText (same turn). Anything else (Divider, ToolCall,
    /// ToolResult, BashExecution, StopNotice, User) is a turn break
    /// and we must start a new line.
    pub fn push_assistant_raw(&mut self, text: impl Into<String>) {
        let s: String = text.into();
        if s.is_empty() {
            return;
        }
        let same_turn = matches!(
            self.transcript_state.lines.last(),
            Some(TranscriptLine::AssistantText(_) | TranscriptLine::ThinkingText(_))
        );
        if same_turn {
            if let Some(TranscriptLine::AssistantText(existing)) =
                Arc::make_mut(&mut self.transcript_state).lines.last_mut()
            {
                existing.push_str(&s);
                return;
            }
        }
        Arc::make_mut(&mut self.transcript_state)
            .lines
            .push(TranscriptLine::AssistantText(s));
    }

    /// v0.8: push reasoning content. Rendered dim/italic.
    ///
    /// v0.8.4 (bugfix, ux-001): same reasoning as push_assistant_raw.
    /// Streaming ThinkingDelta chunks were each pushed as their own
    /// line, so the transcript ended up with hundreds of `💭 The`
    /// `💭 user` `💭 wants` rows — the viewport only showed the last
    /// few chars (`💭 ommand.`). Now we APPEND to the last
    /// ThinkingText line.
    pub fn push_thinking_raw(&mut self, text: impl Into<String>) {
        let s: String = text.into();
        if s.is_empty() {
            return;
        }
        if let Some(TranscriptLine::ThinkingText(existing)) =
            Arc::make_mut(&mut self.transcript_state).lines.last_mut()
        {
            existing.push_str(&s);
        } else {
            Arc::make_mut(&mut self.transcript_state)
                .lines
                .push(TranscriptLine::ThinkingText(s));
        }
    }

    pub fn push_tool_call(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        args: impl Into<String>,
    ) {
        Arc::make_mut(&mut self.transcript_state)
            .lines
            .push(TranscriptLine::ToolCall {
                id: id.into(),
                name: name.into(),
                args: args.into(),
                collapsed: false,
            });
    }

    pub fn push_tool_result(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        ok: bool,
        content: impl Into<String>,
        duration_ms: Option<u64>,
    ) {
        self.push_tool_result_raw(id, name, ok, content, duration_ms);
        // Tool results are part of the turn — session_append is handled on TurnEnd
        // by the session flush, so we do NOT append here to avoid double-logging.
    }

    /// Internal: push to transcript without session logging.
    pub fn push_tool_result_raw(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        ok: bool,
        content: impl Into<String>,
        duration_ms: Option<u64>,
    ) {
        Arc::make_mut(&mut self.transcript_state)
            .lines
            .push(TranscriptLine::ToolResult {
                id: id.into(),
                name: name.into(),
                ok,
                content: content.into(),
                collapsed: false,
                duration_ms,
            });
    }

    /// v0.8.9 (P2-9): surface a condition the user should know about
    /// (a damaged session file, a skipped record) inline in the
    /// transcript, next to the content it affects — not only in a log
    /// nobody reads.
    pub fn push_stop_notice(&mut self, msg: impl Into<String>) {
        Arc::make_mut(&mut self.transcript_state)
            .lines
            .push(TranscriptLine::StopNotice(msg.into()));
    }

    /// v0.10.3: a system notice the TUI produced itself (a line the
    /// library wrote to stderr while the TUI owned the terminal, a
    /// `/copy` fallback, a settings write that failed).
    ///
    /// It is a `Notice` and not a `StopNotice` because it says nothing
    /// about how a turn ended — it is the TUI talking, and it must not
    /// be mistaken for a model message in the export or the search index.
    pub fn push_notice(&mut self, msg: impl Into<String>) {
        Arc::make_mut(&mut self.transcript_state)
            .lines
            .push(TranscriptLine::Notice(msg.into()));
    }

    /// v0.10.x: append a **batch** of system notices in one go.
    ///
    /// The terminal guard hands the render loop a `Vec<String>` of
    /// pending lines (a tool that writes 200 lines to stderr produces
    /// 200 of them), and the loop used to push them one at a time:
    ///
    /// ```text
    /// for line in g.take_pending() {
    ///     push_notice(&shared, line);   // one Arc::make_mut per line
    /// }
    /// ```
    ///
    /// The batch form does one `Arc::make_mut` for the whole batch.
    ///
    /// How much that saves is **not** a measured number, and the
    /// difference matters: `Arc::make_mut` copies the transcript only
    /// when something else still holds a clone of it. In the drain path
    /// the render loop's snapshot from the previous iteration has
    /// already been dropped, so the refcount is 1 and **no copy
    /// happens at all** — neither before this change nor after. An
    /// earlier version of this comment claimed that every push
    /// deep-copied the whole transcript and that a 200-line drain froze
    /// the UI; that was wrong, and it was wrong in the direction that
    /// made the bug sound worse than it was.
    ///
    /// What the batch form *does* buy is an upper bound: at most one
    /// copy-on-write per drain instead of one per line, and one
    /// operation on the transcript instead of N interleaved with the
    /// agent's own messages.
    pub fn push_notices(&mut self, msgs: impl IntoIterator<Item = String>) {
        let transcript = Arc::make_mut(&mut self.transcript_state);
        transcript
            .lines
            .extend(msgs.into_iter().map(TranscriptLine::Notice));
    }

    pub fn push_divider(&mut self) {
        Arc::make_mut(&mut self.transcript_state)
            .lines
            .push(TranscriptLine::Divider);
    }

    /// Rows one page of transcript scrolling covers, as published by the
    /// render path. Falls back to 1 before the first frame.
    pub fn page_lines(&self) -> usize {
        self.page_lines.load(Ordering::Relaxed).max(1)
    }

    /// How many **rendered rows** the transcript viewport is above the live
    /// tail, as published by the render path.
    ///
    /// This is the scrollbar's unit. `transcript_state.scroll_offset` is
    /// *not*: that counts `TranscriptLine`s, and one line can be many
    /// rows tall. The scrollbar's geometry must be computed from this
    /// number, or the thumb the user sees and the thumb the mouse hits
    /// are two different things.
    pub fn items_skipped_from_bottom(&self) -> usize {
        self.items_skipped_from_bottom.load(Ordering::Relaxed)
    }

    /// Record that the user just scrolled, so the scrollbar's `auto` mode
    /// stays lit for `TRANSIENT_HIDE_MS`.
    pub fn mark_scroll_activity(&self) {
        self.scroll_activity
            .store(crate::scrollbar::now_millis(), Ordering::Relaxed);
    }

    /// Milliseconds since the last scroll action.
    pub fn scroll_activity_ms(&self) -> u64 {
        self.scroll_activity.load(Ordering::Relaxed)
    }

    pub fn transcript_len(&self) -> usize {
        self.transcript_state.lines.len()
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
        if self.run_state.context_window == 0 {
            return false;
        }
        let budget = self
            .run_state
            .context_window
            .saturating_sub(settings.reserve_tokens);
        let estimated = self.estimate_transcript_tokens();
        estimated > budget
    }

    /// Sum of (chars / 4) across every text-bearing transcript line.
    /// Used by [`should_auto_compact`] and shown in the status bar.
    pub fn estimate_transcript_tokens(&self) -> u32 {
        let mut total: u32 = 0;
        for line in &self.transcript_state.lines {
            use crate::state::TranscriptLine;
            match line {
                TranscriptLine::User(s) => total += chars_to_tokens(s),
                TranscriptLine::AssistantText(s) => total += chars_to_tokens(s),
                // v0.8: thinking content counts toward context
                // because the model sees its own reasoning on the
                // next turn (it's in the conversation history).
                TranscriptLine::ThinkingText(s) => total += chars_to_tokens(s),
                TranscriptLine::ToolCall { name, args, .. } => {
                    total += chars_to_tokens(name) + chars_to_tokens(args);
                }
                TranscriptLine::ToolResult { content, .. } => total += chars_to_tokens(content),
                TranscriptLine::BashExecution { cmd, output, .. } => {
                    total += chars_to_tokens(cmd) + chars_to_tokens(output);
                }
                TranscriptLine::StopNotice(s) => total += chars_to_tokens(s),
                // A notice is a system message, not model context: it
                // never goes back to the provider, so it does not count
                // toward the context window.
                TranscriptLine::Notice(_) => {}
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
    /// The session's live context as provider messages.
    ///
    /// This is what an Agent should be seeded with. Empty when there is no
    /// session — `--no-session`, or before one is initialised — and an
    /// empty seed is correct there: there is no history to replay.
    ///
    /// v0.10 (fix): the empty `Vec` here used to have two meanings, and the
    /// model could not tell them apart: "this session has no history" and
    /// "another thread held the lock". Contention is not an answer, so the
    /// second case now waits (`lock_session`) and the seed keeps the
    /// conversation.
    pub fn session_context_messages(&self) -> Vec<nini_core::provider::Message> {
        let Some(arc) = self.session_state.session.as_ref() else {
            return Vec::new();
        };
        let guard = lock_session(arc);
        nini_session::build_context_entries(&guard)
            .iter()
            .filter_map(nini_core::conversion::session_entry_to_llm_message)
            .collect()
    }

    /// Re-derive `run_state.context_window` from the settings snapshot.
    ///
    /// v0.10 (R2c): this value was never set in production. `AppState::new`
    /// is what `runtime::run` builds (`runtime.rs:364`), it leaves
    /// `run_state.context_window` at 0, `set_status_bar_usage` has no caller
    /// anywhere in the crate, and `should_auto_compact` opens with
    ///
    /// ```text
    /// if self.run_state.context_window == 0 { return false }
    /// ```
    ///
    /// So the automatic compaction path that R2a/A2 built - persisted,
    /// reconstructed, rebuilt into the transcript - never fired once in a
    /// real session. Every test of it set the window explicitly, which is
    /// exactly how the gap survived.
    ///
    /// The source is, in order: the model catalog's own `contextWindow`
    /// for the selected model, then `settings.context_window` (default
    /// 200_000, mirroring Pi's `DEFAULT_COMPACTION_SETTINGS`).
    ///
    /// The catalog was deliberately ignored when it held a flat 128_000 for
    /// every model - a guess, and a wrong number fires the trigger at the
    /// wrong time. R2d generated it from Pi's own data files, so it now
    /// carries measured values (16 distinct windows across the providers
    /// nini supports) and takes precedence.
    ///
    /// A settings value of 0 stays 0: "unknown window" means no automatic
    /// compaction, and the status bar says so rather than inventing a
    /// number.
    pub fn refresh_context_window(&mut self) {
        // v0.10 (R2d): the catalog now carries Pi's own per-model values
        // (`contextWindow` in `providers/data/*.json`), so for a model it
        // knows, that number wins - it is measured, and settings is a
        // single default the user may not have adjusted for this model.
        //
        // Settings remain the fallback for a model the catalog does not
        // have, and for the case where nobody has said anything at all.
        let from_catalog = nini_core::model_lookup::context_window_for(&self.model_state.model);
        // A model the user's own `models.json` declares (a gateway model, a
        // self-hosted endpoint) is not in the shipped catalog — but the
        // user wrote its window down, and that number beats the 200 000
        // fallback. It is the user's fact about their setup, not a guess.
        let from_user = self
            .model_state
            .user_models
            .iter()
            .find(|m| {
                m.id == self.model_state.model
                    || m.id.ends_with(&format!("/{}", self.model_state.model))
            })
            .map(|m| m.context_window)
            .filter(|w| *w > 0);
        self.run_state.context_window = match from_catalog {
            Some(w) => w,
            None => match from_user {
                Some(w) => w,
                None => self.model_state.settings_snapshot.context_window,
            },
        };
    }

    /// Change the selected model, keeping the context window consistent.
    ///
    /// The two model-change sites in `runtime.rs` assigned
    /// `model_state.model` directly, so anything derived from it went
    /// stale. Routing both through here means there is one place to forget.
    pub fn set_model(&mut self, model: impl Into<String>) {
        let model = model.into();
        self.model_state.model = model;
        // If the provider could not be started, the model name is a
        // lie: the app is talking to the scripted fixture, and no model
        // will answer. The status bar must not go back to looking
        // normal just because the user picked a model — and the user
        // has to be told again, because the first notice has long
        // since scrolled away.
        if let Some(reason) = self.provider_fallback_reason.clone() {
            self.model_state.model = "no model".to_string();
            self.push_notice(format!(
                "still on the scripted fixture provider ({reason}); no model \
                 will answer until the provider is fixed"
            ));
        }
        self.refresh_context_window();
    }

    /// Compact the session for real, and refresh the transcript to match.
    ///
    /// Replaces `auto_compact_local`, which folded the first half of the
    /// *transcript lines* at `total / 2`: no token budget, no session
    /// write, nothing the model or a restart would ever see. The cut now
    /// comes from the same token-budget algorithm the agent loop uses, and
    /// the result is a real `CompactionEntry`.
    ///
    /// Returns the report so the caller can say what happened.
    pub fn session_compact(
        &mut self,
        keep_recent_tokens: u32,
    ) -> Option<nini_session::CompactionReport> {
        let arc = self.session_state.session.as_ref()?.clone();
        let report = {
            let mut guard = lock_session(&arc);
            guard.compact(keep_recent_tokens)
        }?;
        self.transcript_from_context();
        Some(report)
    }

    /// Rebuild the transcript from the session's live context.
    ///
    /// After a compaction the displayed history and the stored history are
    /// two different things unless one is derived from the other. Pi
    /// derives the model context from the entry tree; the transcript has to
    /// follow the same source or the user is shown a history the agent
    /// does not have.
    pub fn transcript_from_context(&mut self) {
        let Some(arc) = self.session_state.session.as_ref().cloned() else {
            return;
        };
        let guard = lock_session(&arc);
        let context = nini_session::build_context_entries(&guard);
        let lines = render_context_entries(&context);
        drop(guard);

        Arc::make_mut(&mut self.transcript_state).lines = lines;
        // The line count just changed wholesale, so any cached row count
        // now describes a different transcript.
        self.invalidate_item_counts();
        Arc::make_mut(&mut self.transcript_state).reset_view();
    }

    /// Toggle the `collapsed` flag on the transcript line at `index` if
    /// that line is collapsible (ToolCall / ToolResult / BashExecution).
    /// Returns true if the toggle changed the line's state.
    pub fn toggle_collapsed(&mut self, index: usize) -> bool {
        if let Some(line) = Arc::make_mut(&mut self.transcript_state)
            .lines
            .get_mut(index)
        {
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
        for line in Arc::make_mut(&mut self.transcript_state).lines.iter_mut() {
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
                self.ui_state.completion = None;
            } else {
                let new_names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
                let prev_selected_name = self
                    .ui_state
                    .completion
                    .as_ref()
                    .and_then(|p| p.current())
                    .map(|i| i.name.clone());
                let new_selection = prev_selected_name
                    .as_ref()
                    .and_then(|n| new_names.iter().position(|m| m == n))
                    .unwrap_or(0);
                self.ui_state.completion = Some(CompletionPopup {
                    items,
                    selected: new_selection,
                    scroll_offset: 0,
                    max_visible: DEFAULT_COMPLETION_VISIBLE,
                });
            }
            return;
        }
        // @ file-path completion.
        if let Some(path_prefix) =
            crate::file_completion::extract_at_prefix(&self.input.text, self.input.cursor)
        {
            let cwd = std::env::current_dir().unwrap_or_default();
            let files = crate::file_completion::search_files_as_struct(&cwd, &path_prefix);
            if files.is_empty() {
                self.ui_state.completion = None;
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
            self.ui_state.completion = Some(CompletionPopup {
                items,
                selected: 0,
                scroll_offset: 0,
                max_visible: DEFAULT_COMPLETION_VISIBLE,
            });
            return;
        }
        self.ui_state.completion = None;
    }

    /// Apply the currently-selected completion to the input buffer.
    /// Replaces the partial command name with the full one.
    pub fn apply_completion(&mut self) {
        let Some(popup) = &self.ui_state.completion else {
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
        self.ui_state.completion = None;
    }

    /// Begin a transcript search. Called when the user presses `/` in
    /// editing mode (with no slash-command popup showing).
    pub fn begin_search(&mut self) {
        self.ui_state.search = Some(SearchState {
            query: String::new(),
            matches: Vec::new(),
            current: 0,
        });
    }

    /// Update the search query and recompute matches against the
    /// current transcript. Case-insensitive substring match.
    pub fn update_search_query(&mut self, q: String) {
        let Some(s) = self.ui_state.search.as_mut() else {
            return;
        };
        s.query = q.clone();
        let q_lower = q.to_lowercase();
        let mut new_matches: Vec<usize> = Vec::new();
        for (i, line) in self.transcript_state.lines.iter().enumerate() {
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
        // v0.10.2: the index changed, so the viewport has to follow. See
        // `reveal_search_match`.
        self.reveal_search_match();
    }

    /// Bring the current search match into the viewport.
    ///
    /// v0.10.2. This is the missing half of search: `search_next` moved
    /// `SearchState::current` and nothing wrote `scroll_offset`, while
    /// `autoscroll` kept the transcript pinned to the tail. The user
    /// typed a query, read `2/2`, pressed `n`, and the screen did not
    /// change by a single cell.
    ///
    /// `scroll_offset` counts **TranscriptLines skipped from the bottom**,
    /// which is what the renderer translates into an item-space skip. So
    /// putting line `i` at the top of the viewport means skipping every
    /// line below it: `total - 1 - i`.
    ///
    /// Deliberately *not* "scroll only if the match is already off
    /// screen": the renderer does not report what is visible back into
    /// state, and re-deriving it here would duplicate the item-count
    /// cache that only the renderer owns. Landing on the match every
    /// time is also the more predictable contract — after `n`, the user
    /// knows the new match is at the top of the screen.
    fn reveal_search_match(&mut self) {
        let Some(s) = self.ui_state.search.as_ref() else {
            return;
        };
        let Some(&idx) = s.matches.get(s.current) else {
            return;
        };
        let total = self.transcript_state.lines.len();
        if idx >= total {
            return;
        }
        // Leaving autoscroll on would ignore scroll_offset entirely
        // (render_transcript takes the tail-aligned branch), so the flag
        // has to come off for a search to be able to look backwards.
        Arc::make_mut(&mut self.transcript_state).autoscroll = false;
        Arc::make_mut(&mut self.transcript_state).scroll_offset = total.saturating_sub(1 + idx);
    }

    /// Advance to the next match (wraps at the end).
    pub fn search_next(&mut self) {
        if let Some(s) = self.ui_state.search.as_mut() {
            if !s.matches.is_empty() {
                s.current = (s.current + 1) % s.matches.len();
            }
        }
        self.reveal_search_match();
    }

    /// Advance to the previous match (wraps at the start).
    pub fn search_prev(&mut self) {
        if let Some(s) = self.ui_state.search.as_mut() {
            if !s.matches.is_empty() {
                s.current = if s.current == 0 {
                    s.matches.len() - 1
                } else {
                    s.current - 1
                };
            }
        }
        self.reveal_search_match();
    }

    /// Close the search overlay and clear highlights.
    ///
    /// v0.10.2: the comment said "clear highlights" and the body did not
    /// — there was nothing to clear, because nothing painted one. Now
    /// there is, and the transcript also has to come back to the tail:
    /// `reveal_search_match` turned `autoscroll` off to look backwards,
    /// and leaving it off would strand the user scrolled into the past
    /// after they closed the search.
    pub fn end_search(&mut self) {
        self.ui_state.search = None;
        Arc::make_mut(&mut self.transcript_state).reset_view();
    }
}

fn chars_to_tokens(s: &str) -> u32 {
    ((s.chars().count() as u32) + 3) / 4
}

/// Public summary form of a transcript line, used by the transcript
/// search (F019) to do case-insensitive substring matches. We
/// intentionally include prefixes like `[user]` so users searching
/// for "tool-call" can find the right lines without guessing the
/// underlying text.
///
/// **v0.10.2: nothing is truncated here any more.** This function is
/// the *only* input `update_search_query` matches against, and it used
/// to end every assistant / thinking / tool-result line at 60
/// characters and every tool-call argument at 40. The comment above it
/// called the result "suitable for transcript search" — and a search
/// index that silently drops 90% of a typical model answer is not a
/// search.
///
/// The cost of removing the cap is one `to_lowercase()` + `contains()`
/// over the line's own text per keystroke. A 200-turn transcript is on
/// the order of 10^5 characters; that is microseconds, and it replaced
/// a feature that did not work.
///
/// `truncate` is kept below because it still has one caller.
pub(crate) fn line_summary_text_impl(line: &crate::state::TranscriptLine) -> String {
    use crate::state::TranscriptLine;
    let flat = |s: &str| s.replace('\n', " ");
    match line {
        TranscriptLine::User(s) => format!("[user] {}", flat(s)),
        TranscriptLine::AssistantText(s) => format!("[assistant] {}", flat(s)),
        TranscriptLine::ToolCall { name, args, .. } => {
            format!("[tool-call] {name}({})", flat(args))
        }
        TranscriptLine::ToolResult { content, .. } => format!("[tool-result] {}", flat(content)),
        TranscriptLine::BashExecution { cmd, output, .. } => {
            format!("[bash] {cmd} -> {}", flat(output))
        }
        TranscriptLine::Divider => "[divider]".to_string(),
        TranscriptLine::ThinkingText(s) => format!("[thinking] {}", flat(s)),
        TranscriptLine::StopNotice(s) => format!("[stop] {s}"),
        TranscriptLine::Notice(s) => format!("[notice] {s}"),
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
        s.push_tool_call("t1", "bash", "{}");
        s.push_tool_result("t1", "bash", true, "ok", None);
        s.push_divider();
        assert_eq!(s.transcript_len(), 5);
    }

    #[test]
    fn toggle_collapsed_flips_tool_call_state() {
        let mut s = AppState::new("test");
        s.push_tool_call("t1", "bash", "{}");
        // ToolCall index is 0.
        assert!(s.toggle_collapsed(0));
        // Verify the line is now collapsed by re-pushing another line
        // and reading back the transcript — collapsed flag persists.
        s.push_divider();
        match &s.transcript_state.lines[0] {
            TranscriptLine::ToolCall { collapsed, .. } => assert!(*collapsed),
            other => panic!("expected ToolCall, got {other:?}"),
        }
        // Toggle back to expanded.
        assert!(s.toggle_collapsed(0));
        match &s.transcript_state.lines[0] {
            TranscriptLine::ToolCall { collapsed, .. } => assert!(!(*collapsed)),
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn toggle_collapsed_flips_tool_result_and_bash() {
        let mut s = AppState::new("test");
        s.push_tool_result("t1", "bash", true, "ok", None);
        s.push_tool_call("t2", "read", "{}");
        // Append a bash via the underlying TranscriptLine constructor
        // since we don't have a public push_bash helper yet.
        Arc::make_mut(&mut s.transcript_state)
            .lines
            .push(TranscriptLine::BashExecution {
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
        match &s.transcript_state.lines[0] {
            TranscriptLine::ToolResult { collapsed, .. } => assert!(*collapsed),
            _ => panic!(),
        }
        match &s.transcript_state.lines[1] {
            TranscriptLine::ToolCall { collapsed, .. } => assert!(*collapsed),
            _ => panic!(),
        }
        match &s.transcript_state.lines[2] {
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
        s.push_tool_call("t1", "bash", "{}");
        assert!(!s.toggle_collapsed(99));
    }

    #[test]
    fn estimate_transcript_tokens_sums_text_lines() {
        let mut s = AppState::new("m");
        s.push_user("hello"); // 5 chars -> 2 tokens
        s.push_assistant("world this is longer"); // 21 -> 6 tokens
        s.push_tool_result("t1", "bash", true, "out", None);
        // total chars / 4 rounded up
        assert!(s.estimate_transcript_tokens() > 0);
    }

    #[test]
    fn should_auto_compact_returns_false_when_window_zero() {
        // No context_window set: never auto-compact (user hasn't
        // configured a model size yet).
        let mut s = AppState::new("m");
        s.run_state.context_window = 0;
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
    fn collapse_all_folds_every_collapsible_line() {
        let mut s = AppState::new("test");
        s.push_tool_call("t1", "bash", "{}");
        s.push_tool_result("t1", "bash", true, "ok", None);
        s.push_divider();
        s.push_user("hi");
        s.push_assistant("hello");
        // Index 5: a bash via direct TranscriptLine.
        Arc::make_mut(&mut s.transcript_state)
            .lines
            .push(TranscriptLine::BashExecution {
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
        for (i, line) in s.transcript_state.lines.iter().enumerate() {
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
        let search = s.ui_state.search.as_ref().expect("search active");
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
        assert!(s.ui_state.search.as_ref().unwrap().matches.is_empty());
    }

    #[test]
    fn search_next_advances_with_wrap() {
        let mut s = AppState::new("test");
        for i in 0..3 {
            s.push_user(format!("line {i} match"));
        }
        s.begin_search();
        s.update_search_query("match".to_string());
        assert_eq!(s.ui_state.search.as_ref().unwrap().matches.len(), 3);
        // Initially at index 0; search_next → 1 → 2 → wraps to 0.
        s.search_next();
        assert_eq!(s.ui_state.search.as_ref().unwrap().current, 1);
        s.search_next();
        assert_eq!(s.ui_state.search.as_ref().unwrap().current, 2);
        s.search_next();
        assert_eq!(s.ui_state.search.as_ref().unwrap().current, 0);
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
        assert_eq!(s.ui_state.search.as_ref().unwrap().current, 2);
    }

    #[test]
    fn end_search_clears_state() {
        let mut s = AppState::new("test");
        s.push_user("hello");
        s.begin_search();
        s.update_search_query("hello".to_string());
        s.end_search();
        assert!(s.ui_state.search.is_none());
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

#[cfg(test)]
mod sub_struct_tests {
    //! v0.8.1: per-sub-struct unit tests. Verifies each sub-struct
    //! can be constructed, cloned, and mutated independently —
    //! i.e. the AppState split didn't introduce cross-coupling.

    use super::*;

    #[test]
    fn transcript_state_push_user_increments_length() {
        let mut ts = TranscriptState::default();
        for i in 0..3 {
            ts.lines.push(TranscriptLine::User(format!("msg {i}")));
        }
        assert_eq!(ts.lines.len(), 3);
        assert!(matches!(ts.lines[0], TranscriptLine::User(_)));
    }

    #[test]
    fn transcript_state_estimate_tokens_simple() {
        let mut ts = TranscriptState::default();
        // 8 chars / 4 = 2 tokens (round up)
        ts.lines
            .push(TranscriptLine::AssistantText("abcdefgh".into()));
        let app = AppState {
            transcript_state: Arc::new(ts),
            ..AppState::default()
        };
        assert_eq!(app.estimate_transcript_tokens(), 2);
    }

    #[test]
    fn run_state_mode_transitions() {
        let mut rs = RunState::default();
        assert_eq!(rs.mode, RunMode::Editing);
        rs.mode = RunMode::Running;
        assert_eq!(rs.mode, RunMode::Running);
        rs.mode = RunMode::Editing;
        assert_eq!(rs.mode, RunMode::Editing);
    }

    #[test]
    fn run_state_pending_quit_window() {
        let mut rs = RunState::default();
        rs.pending_quit = Some(std::time::Instant::now());
        assert!(rs.pending_quit.is_some());
        rs.pending_quit = None;
        assert!(rs.pending_quit.is_none());
    }

    #[test]
    fn model_state_cycle_model_advances() {
        let mut ms = ModelState::default();
        ms.models_cycle = vec!["a".into(), "b".into(), "c".into()];
        ms.models_cycle_idx = Some(0);
        // Cycle forward
        let next = (ms.models_cycle_idx.unwrap() + 1) % ms.models_cycle.len();
        ms.models_cycle_idx = Some(next);
        assert_eq!(ms.models_cycle_idx, Some(1));
        // Wrap-around at end
        ms.models_cycle_idx = Some(2);
        let next = (ms.models_cycle_idx.unwrap() + 1) % ms.models_cycle.len();
        ms.models_cycle_idx = Some(next);
        assert_eq!(ms.models_cycle_idx, Some(0));
    }

    #[test]
    fn model_state_provider_roundtrip() {
        let mut ms = ModelState::default();
        ms.provider = Some("anthropic".into());
        assert_eq!(ms.provider.as_deref(), Some("anthropic"));
        ms.provider = None;
        assert_eq!(ms.provider, None);
    }

    #[test]
    fn session_state_metadata_setter() {
        let mut ss = SessionState::default();
        ss.cwd = Some(PathBuf::from("/tmp"));
        ss.git_branch = Some("main".into());
        assert_eq!(ss.cwd.as_deref(), Some(std::path::Path::new("/tmp")));
        assert_eq!(ss.git_branch.as_deref(), Some("main"));
    }

    #[test]
    fn ui_state_selector_clone_drops() {
        // The selector field holds Option<Box<dyn SelectorState>>,
        // which is not Clone. UiState::clone() must reset selector
        // to None (mirrors v0.8.0's clone_for_render behavior).
        let mut us = UiState::default();
        us.theme_name = Some("dark".into());
        let cloned = us.clone();
        assert_eq!(cloned.theme_name, Some("dark".into()));
        assert!(cloned.selector.is_none());
    }

    #[test]
    fn appstate_default_has_six_fields() {
        let s = AppState::default();
        assert_eq!(s.input.text, ""); // InputBuffer::new()
        assert!(s.transcript_state.lines.is_empty());
        assert_eq!(s.run_state.mode, RunMode::Editing);
        assert_eq!(s.model_state.model, "");
        assert!(s.session_state.session.is_none());
        assert!(s.ui_state.selector.is_none());
    }
}
