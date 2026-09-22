//! TreeSelector — visualize a session tree and let the user pick a node
//! to navigate to (used by /tree and /fork).
//!
//! Each entry becomes a row. BranchSummaryEntry / CompactionEntry are
//! shown with an icon prefix; assistant messages with the first 60 chars
//! of text. The selector returns the picked entry id; /fork then forks
//! at that point.

use std::any::Any;

use nini_session::SessionEntry;

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
                    id: e.id().to_string(),
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
        },
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
                self.result = Some(it.id.clone());
                SelectorOutcome::Picked(it)
            }
            None => SelectorOutcome::Cancelled,
        }
    }
}
