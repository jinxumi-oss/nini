//! TreeSelector — visualize a session tree and let the user pick a node
//! to navigate to (used by /tree and /fork).
//!
//! Each entry becomes a row. BranchSummaryEntry / CompactionEntry are
//! shown with an icon prefix; assistant messages with the first 60 chars
//! of text. The selector returns the picked entry id; /fork then forks
//! at that point.

use std::any::Any;

use nini_session::SessionEntry;
use unicode_width::UnicodeWidthChar;

use crate::selector::{SelectorItem, SelectorOutcome, SelectorState};

#[derive(Debug, Clone)]
pub struct TreeSelector {
    pub result: Option<String>,
    items: Vec<SelectorItem>,
    selected: usize,
}

impl TreeSelector {
    pub fn from_entries(entries: &[SessionEntry]) -> Self {
        let items: Vec<SelectorItem> = entries
            .iter()
            .map(|e| {
                let (icon, label) = label_for(e);
                SelectorItem {
                    value: e.id().to_string(),
                    label: format!("{icon} {label}"),
                    description: None,
                    is_current: false,
                }
            })
            .collect();
        Self {
            result: None,
            items,
            selected: 0,
        }
    }

    pub fn summarize_at(&mut self, _idx: usize, _entries: &[SessionEntry]) -> Option<String> {
        None
    }
}

fn label_for(entry: &SessionEntry) -> (&'static str, String) {
    match entry {
        SessionEntry::BranchSummary(b) => ("◇", format!("branch: {}", truncate(&b.summary, 40))),
        SessionEntry::Compaction(b) => ("◆", format!("compaction: {}", truncate(&b.summary, 40))),
        SessionEntry::ModelChange(m) => ("·", format!("model → {}", m.model_id)),
        SessionEntry::ThinkingLevelChange(t) => ("·", format!("thinking → {}", t.thinking_level)),
        SessionEntry::Label(l) => ("✎", format!("label: {}", l.label.as_deref().unwrap_or(""))),
        SessionEntry::SessionInfo(s) => ("ℹ", format!("name: {}", s.name)),
        SessionEntry::Custom(c) => ("·", format!("custom: {}", c.custom_type)),
        SessionEntry::CustomMessage(_) => ("·", "[custom msg]".to_string()),
        SessionEntry::Message(m) => match m.message {
            nini_session::AgentMessage::User(_) => ("›", "[user]".to_string()),
            nini_session::AgentMessage::Assistant(_) => ("•", "[assistant]".to_string()),
            nini_session::AgentMessage::ToolResult(_) => ("✗", "[tool result]".to_string()),
            nini_session::AgentMessage::Custom(_) => ("·", "[custom]".to_string()),
            nini_session::AgentMessage::BashExecution(_) => ("$", "[bash]".to_string()),
            nini_session::AgentMessage::BranchSummary(_) => ("◇", "[branch summary]".to_string()),
            nini_session::AgentMessage::CompactionSummary(_) => ("◆", "[compaction summary]".to_string()),
            // v0.7 (M3a) internal-only variants — these never reach
            // the LLM but they DO appear in the session log. Render
            // them as dim metadata in the tree view.
            nini_session::AgentMessage::Notification(_) => ("·", "[notification]".to_string()),
            nini_session::AgentMessage::UiMessage(_) => ("·", "[ui message]".to_string()),
            nini_session::AgentMessage::AppMessage(_) => ("·", "[app message]".to_string()),
        },
    }
}

fn truncate(s: &str, max: usize) -> String {
    // Truncate by display width so an emoji or CJK character is never
    // cut mid-glyph (the previous `chars().take()` counted code points,
    // not cells, so we'd cut mid-emoji and the renderer would show a
    // tofu glyph).
    if crate::width::display_width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for c in s.chars() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w + 1 > max {
            // +1 reserves a cell for the trailing ellipsis.
            out.push('\u{2026}');
            return out;
        }
        out.push(c);
        used += w;
    }
    out
}

impl SelectorState for TreeSelector {
    fn as_any_mut(&mut self) -> &mut dyn Any { self }
    fn state_items(&self) -> Vec<SelectorItem> { self.items.clone() }
    fn state_title(&self) -> String { "Session tree".to_string() }
    fn state_selected(&self) -> usize { self.selected }
    fn state_set_selected(&mut self, idx: usize) {
        if idx < self.items.len() {
            self.selected = idx;
        }
    }
    fn state_on_select(&mut self) -> SelectorOutcome {
        let item = self.items.get(self.selected).cloned();
        match item {
            Some(it) => {
                self.result = Some(it.value.clone());
                SelectorOutcome::Picked(it)
            }
            None => SelectorOutcome::Cancelled,
        }
    }
}
