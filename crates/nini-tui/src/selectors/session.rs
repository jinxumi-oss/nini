//! SessionSelector — picks a session JSONL file from the sessions directory.
//!
//! If `from_dir` is given, lists sessions in that directory; otherwise
//! defaults to `~/.pi/agent/sessions/`. Each item's label is the relative
//! timestamp from the filename; description is the parsed cwd from the
//! session header if readable.

use std::any::Any;
use std::path::{Path, PathBuf};

use crate::selector::{SelectorItem, SelectorOutcome, SelectorState};

#[derive(Debug, Clone)]
pub struct SessionSelector {
    pub result: Option<String>,
    items: Vec<SelectorItem>,
    selected: usize,
}

impl SessionSelector {
    pub fn from_dir<D: AsRef<std::path::Path>>(dir: D) -> Self {
        let dir = dir.as_ref().to_path_buf();
        let items = list_sessions(&dir);
        let result = items.first().map(|i| i.id.clone());
        Self {
            result,
            items,
            selected: 0,
        }
    }
}

fn list_sessions(dir: &Path) -> Vec<SelectorItem> {
    let mut items = Vec::new();
    let read_dir = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return items,
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let id = path.to_string_lossy().to_string();
        let label = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| id.clone());
        items.push(SelectorItem {
            id,
            label,
            description: None,
            is_current: false,
        });
    }
    // Sort by filename (timestamps are in the name) so newest is first.
    items.sort_by(|a, b| b.label.cmp(&a.label));
    items
}

impl SelectorState for SessionSelector {
    fn as_any_mut(&mut self) -> &mut dyn Any { self }
    fn state_items(&self) -> Vec<SelectorItem> { self.items.clone() }
    fn state_title(&self) -> String { "Resume session".to_string() }
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
