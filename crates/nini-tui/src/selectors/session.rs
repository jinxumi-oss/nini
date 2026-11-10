//! SessionSelector — picks a session JSONL file out of the sessions tree.
//!
//! The rows have to be **recognisable at a glance**: the label is the
//! session's own name, or failing that the first thing the user said, and
//! the description carries the relative time plus the project directory the
//! session belongs to. The previous rows were the bare UUID filename with no
//! description at all — which is exactly why a listing of them was a wall of
//! text nobody could tell apart.
//!
//! # Pi parity (B35)
//!
//! Pi's session selector
//! (`@earendil-works/pi-coding-agent/dist/modes/interactive/components/session-selector.js`)
//! offers four things nini did not have:
//!
//! * **scope** — the current folder or every session (`Tab`),
//! * **sort modes** — threaded / recent / relevance (`Ctrl+S`),
//! * **delete** — with a confirmation, the active session excluded
//!   (`Ctrl+D`),
//! * **rename** — written as an appended `session_info` entry (`Ctrl+R`),
//!
//! plus a *named sessions only* filter (`Ctrl+N`) and the path column
//! (`Ctrl+P`). All of them are here.
//!
//! ## Two deviations from Pi, both deliberate
//!
//! 1. **The scope starts on "all", not "current folder".** Pi starts on
//!    "current" (`session-selector.js:594`), but nini's picker rows are
//!    numbered by the same list `/resume <n>` uses
//!    ([`session_paths`]), and a row number that means a different session
//!    in the two places is worse than no number at all. Starting on "all"
//!    keeps the picker's row *N* the session `/resume N` loads.
//! 2. **The `re:` regex search mode is not implemented.** Pi's
//!    `session-selector-search.js:29` parses `re:<pattern>` as a regular
//!    expression; nini's TUI crate has no regex dependency, and adding one
//!    for a search mode nobody asked for is not worth it. Such a query is
//!    treated as a literal token.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::selector::{SelectorItem, SelectorOutcome, SelectorState};
use crate::selectors::pi_fuzzy::fuzzy_match;

/// How many bytes of a session file are read to learn its name and its
/// first user message. The session format is owned by `nini-session`; this
/// module only asks it for a bounded read (see
/// `nini_session::read_meta_bounded`).
const META_BYTES: usize = 8 * 1024;

/// How many characters of a session's subject the label keeps. The panel's
/// label column is 32 cells wide, but the filter still has to be able to
/// match on more than what fits on screen.
const LABEL_CHARS: usize = 60;

/// Every session file under the sessions root, in the order `/resume <n>`
/// numbers them: the current project's directory first, then everything else
/// by mtime descending.
///
/// This used to be a nested function inside `cmd_resume`
/// (`crates/nini-tui/src/commands/dispatch.rs`), which the selector could
/// not reach. It lives here now because **both** the picker and
/// `/resume <n>` have to show the same order — a number that means a
/// different session in the two places is worse than no number at all.
pub fn session_paths() -> Vec<PathBuf> {
    // The previous version fell back to `$HOME/fallback/sessions` when
    // the agent directory could not be resolved. That path exists
    // nowhere and was never written to; it only ever made the listing
    // look like it was looking somewhere.
    let Some(base) = nini_core::paths::sessions_dir() else {
        return Vec::new();
    };
    if !base.exists() {
        return Vec::new();
    }
    let mut out: Vec<PathBuf> = walkdir::WalkDir::new(&base)
        .max_depth(4)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file() && e.path().extension().map(|s| s == "jsonl").unwrap_or(false)
        })
        .map(|e| e.into_path())
        .collect();
    // Sort: cwd-name subdirectory first (matches the user's
    // current project), then everything else by mtime desc so
    // recent activity is easy to spot.
    let cwd_name = std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    out.sort_by(|a, b| {
        let a_cwd = a.components().any(|c| c.as_os_str() == cwd_name.as_str());
        let b_cwd = b.components().any(|c| c.as_os_str() == cwd_name.as_str());
        match (a_cwd, b_cwd) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => {
                let a_mt = a
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                let b_mt = b
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                b_mt.cmp(&a_mt)
            }
        }
    });
    out
}

/// The sessions in the order the picker shows them.
///
/// `/resume <n>` and the picker's rows must mean the same session, and
/// the picker's default order is Pi's **threaded** one (a fork nests
/// under the session it came from) — so both go through this one
/// function. Reading the headers is the price: a fork's place in the
/// list is only known from the header's `parentSession`.
pub fn picker_order() -> Vec<PathBuf> {
    let rows: Vec<SessionRow> = session_paths().iter().map(|p| row_for(p)).collect();
    let refs: Vec<&SessionRow> = rows.iter().collect();
    threaded_order(&refs)
        .into_iter()
        .map(|v| v.row.path.clone())
        .collect()
}

/// Which sessions the picker lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionScope {
    /// The sessions of the folder nini is running in.
    Current,
    /// Every session under the sessions root.
    All,
}

/// How the rows are ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSort {
    /// Forked sessions nested under the session they came from.
    Threaded,
    /// The order the sessions were loaded in (most recent first).
    Recent,
    /// By how well the row matches the query.
    Relevance,
}

/// Whether unnamed sessions are listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionNameFilter {
    /// Every session.
    All,
    /// Only the sessions that carry a name.
    Named,
}

/// One session, as the picker knows it.
#[derive(Debug, Clone)]
struct SessionRow {
    path: PathBuf,
    /// The session's own name (`session_info`).
    name: Option<String>,
    /// The first line of the session's first user message.
    first_message: Option<String>,
    /// The header's `cwd`.
    cwd: Option<String>,
    /// The header's `id`.
    id: Option<String>,
    /// The header's `parentSession` — the session this one was forked from.
    parent_id: Option<String>,
    /// How many messages the session holds (`None` = unknown).
    message_count: Option<usize>,
    modified: SystemTime,
}

impl SessionRow {
    /// Does this session carry a name of its own?
    fn named(&self) -> bool {
        self.name
            .as_deref()
            .map(|n| !n.trim().is_empty())
            .unwrap_or(false)
    }
}

impl SessionRow {
    /// The row's label: the session's name, else the first thing the user
    /// said, else a note that there is nothing in it.
    fn label(&self) -> String {
        self.name
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| self.first_message.clone().filter(|s| !s.trim().is_empty()))
            .unwrap_or_else(|| "(no messages)".to_string())
            .chars()
            .take(LABEL_CHARS)
            .collect()
    }

    /// The text the search box matches against.
    ///
    /// Pi: `session-selector-search.js#getSessionSearchText` — the id, the
    /// name, the messages and the cwd. nini has the first user message
    /// rather than every message (its listing read is bounded, see
    /// `nini_session::read_meta_bounded`).
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.id.as_deref().unwrap_or(""),
            self.name.as_deref().unwrap_or(""),
            self.first_message.as_deref().unwrap_or(""),
            self.cwd.as_deref().unwrap_or(""),
        )
    }
}

/// A row as it is shown: the session plus its place in the tree.
#[derive(Debug, Clone)]
struct VisibleRow<'a> {
    row: &'a SessionRow,
    depth: usize,
    is_last: bool,
    ancestors: Vec<bool>,
}

impl VisibleRow<'_> {
    /// The tree prefix Pi draws in front of a nested session
    /// (`session-selector.js#buildTreePrefix`).
    fn prefix(&self) -> String {
        if self.depth == 0 {
            return String::new();
        }
        let mut out = String::new();
        for continues in &self.ancestors {
            out.push_str(if *continues { "│  " } else { "   " });
        }
        out.push_str(if self.is_last { "└─ " } else { "├─ " });
        out
    }
}

/// One term of the search box.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SearchToken {
    /// Matched fuzzily, like Pi's plain tokens.
    Fuzzy(String),
    /// Matched exactly, inside quotes.
    Phrase(String),
}

/// Pi's session picker.
#[derive(Debug)]
pub struct SessionSelector {
    pub result: Option<String>,
    /// Every session on disk, in [`session_paths`] order.
    rows: Vec<SessionRow>,
    scope: SessionScope,
    sort: SessionSort,
    name_filter: SessionNameFilter,
    show_path: bool,
    /// The session nini is running right now — it cannot be deleted.
    current_session: Option<PathBuf>,
    /// The session a delete is waiting for a confirmation for.
    confirming: Option<PathBuf>,
    /// The session being renamed, and the name being typed.
    renaming: Option<PathBuf>,
    rename_input: String,
    /// The last thing that happened (an error, or a confirmation).
    status: Option<String>,
    selected: usize,
}

impl SessionSelector {
    /// Every session on disk, in `session_paths()` order.
    ///
    /// `result` starts as `None` **on purpose**: the runtime applies the
    /// selector's result when the panel closes — whether the user pressed
    /// Enter or Esc (`runtime.rs`). The old constructor pre-set the result
    /// to the first item, so pressing Esc silently switched to the first
    /// session in the list. A cancel that loads a session is not a cancel.
    pub fn all_sessions() -> Self {
        let rows = session_paths().iter().map(|p| row_for(p)).collect();
        Self {
            result: None,
            rows,
            scope: SessionScope::All,
            sort: SessionSort::Threaded,
            name_filter: SessionNameFilter::All,
            show_path: false,
            current_session: None,
            confirming: None,
            renaming: None,
            rename_input: String::new(),
            status: None,
            selected: 0,
        }
    }

    /// Tell the picker which session is the running one: it cannot be
    /// deleted (Pi: `session-selector.js:430`).
    pub fn with_current_session(mut self, path: Option<PathBuf>) -> Self {
        self.current_session = path;
        self
    }

    pub fn scope(&self) -> SessionScope {
        self.scope
    }

    pub fn sort_mode(&self) -> SessionSort {
        self.sort
    }

    pub fn name_filter(&self) -> SessionNameFilter {
        self.name_filter
    }

    pub fn show_path(&self) -> bool {
        self.show_path
    }

    /// The session a delete confirmation is waiting for.
    pub fn confirming_delete(&self) -> Option<&Path> {
        self.confirming.as_deref()
    }

    /// The session being renamed, if any.
    pub fn renaming(&self) -> Option<&Path> {
        self.renaming.as_deref()
    }

    /// What the user has typed into the rename box.
    pub fn rename_input(&self) -> &str {
        &self.rename_input
    }

    /// The last status message (an error, or "Session deleted").
    pub fn status_message(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Take the status message — the runtime is what puts it on screen,
    /// and taking it keeps the message from being shown twice.
    pub fn take_status(&mut self) -> Option<String> {
        self.status.take()
    }

    /// Switch between the current folder and every session (Pi: `Tab`).
    pub fn toggle_scope(&mut self) -> bool {
        self.scope = match self.scope {
            SessionScope::Current => SessionScope::All,
            SessionScope::All => SessionScope::Current,
        };
        self.clamp_selection();
        true
    }

    /// Cycle the sort mode (Pi: `Ctrl+S`).
    pub fn toggle_sort(&mut self) -> bool {
        self.sort = match self.sort {
            SessionSort::Threaded => SessionSort::Recent,
            SessionSort::Recent => SessionSort::Relevance,
            SessionSort::Relevance => SessionSort::Threaded,
        };
        self.clamp_selection();
        true
    }

    /// Toggle the "named sessions only" filter (Pi: `Ctrl+N`).
    pub fn toggle_name_filter(&mut self) -> bool {
        self.name_filter = match self.name_filter {
            SessionNameFilter::All => SessionNameFilter::Named,
            SessionNameFilter::Named => SessionNameFilter::All,
        };
        self.clamp_selection();
        true
    }

    /// Toggle the path column (Pi: `Ctrl+P`).
    pub fn toggle_show_path(&mut self) -> bool {
        self.show_path = !self.show_path;
        true
    }

    /// Ask to delete the selected session (Pi: `Ctrl+D`).
    ///
    /// The running session is refused: deleting it would leave nini
    /// pointing at a file that is not there.
    pub fn start_delete(&mut self) -> bool {
        let Some(row) = self.selected_row() else {
            return false;
        };
        if self.current_session.as_deref() == Some(row.path.as_path()) {
            self.status = Some("Cannot delete the currently active session".to_string());
            return false;
        }
        self.confirming = Some(row.path.clone());
        true
    }

    /// Delete the session the confirmation is about (Pi: `tui.select.confirm`).
    ///
    /// Pi tries the `trash` CLI first and falls back to unlinking; without a
    /// trash command that fallback is the whole behaviour, so nini unlinks.
    pub fn confirm_delete(&mut self) -> bool {
        let Some(path) = self.confirming.take() else {
            return false;
        };
        match std::fs::remove_file(&path) {
            Ok(()) => {
                self.rows.retain(|r| r.path != path);
                self.status = Some("Session deleted".to_string());
                true
            }
            Err(e) => {
                self.status = Some(format!("Failed to delete: {e}"));
                false
            }
        }
    }

    /// Drop the delete confirmation (Pi: `tui.select.cancel`).
    pub fn cancel_delete(&mut self) {
        self.confirming = None;
    }

    /// Start renaming the selected session (Pi: `Ctrl+R`).
    pub fn start_rename(&mut self) -> bool {
        let Some((path, name)) = self
            .visible_rows()
            .get(self.selected)
            .map(|v| (v.row.path.clone(), v.row.name.clone()))
        else {
            return false;
        };
        self.renaming = Some(path);
        self.rename_input = name.unwrap_or_default();
        true
    }

    /// A character typed into the rename box.
    pub fn rename_push(&mut self, c: char) {
        self.rename_input.push(c);
    }

    /// Backspace in the rename box.
    pub fn rename_backspace(&mut self) {
        self.rename_input.pop();
    }

    /// Leave the rename mode without renaming anything.
    pub fn cancel_rename(&mut self) {
        self.renaming = None;
        self.rename_input.clear();
    }

    /// Write the new name into the session (Pi: `appendSessionInfo`).
    ///
    /// The session **file** keeps its name — the new name is one more
    /// `session_info` entry in it, exactly as Pi does it.
    pub fn commit_rename(&mut self) -> bool {
        let Some(path) = self.renaming.take() else {
            return false;
        };
        let name = self.rename_input.trim().to_string();
        self.rename_input.clear();
        if name.is_empty() {
            return false;
        }
        match nini_session::append_session_info(&path, &name) {
            Ok(()) => {
                if let Some(row) = self.rows.iter_mut().find(|r| r.path == path) {
                    row.name = Some(name);
                }
                self.status = Some("Session renamed".to_string());
                true
            }
            Err(e) => {
                self.status = Some(format!("Failed to rename: {e}"));
                false
            }
        }
    }

    /// The row the cursor is on.
    fn selected_row(&self) -> Option<&SessionRow> {
        self.visible_rows().get(self.selected).map(|v| v.row)
    }

    /// Keep the cursor on a row that exists.
    fn clamp_selection(&mut self) {
        let len = self.visible_rows().len();
        if self.selected >= len {
            self.selected = len.saturating_sub(1);
        }
    }

    /// The rows the picker shows: the scope and the name filter applied,
    /// then the sort mode's order.
    fn visible_rows(&self) -> Vec<VisibleRow<'_>> {
        let cwd = std::env::current_dir().ok();
        let cwd_name = cwd
            .as_ref()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let kept: Vec<&SessionRow> = self
            .rows
            .iter()
            .filter(|r| self.in_scope(r, &cwd_name, cwd.as_deref()))
            .filter(|r| self.name_filter == SessionNameFilter::All || r.named())
            .collect();
        match self.sort {
            SessionSort::Threaded => self.threaded(kept),
            SessionSort::Recent | SessionSort::Relevance => kept
                .into_iter()
                .map(|row| VisibleRow {
                    row,
                    depth: 0,
                    is_last: true,
                    ancestors: Vec::new(),
                })
                .collect(),
        }
    }

    /// Is this session part of the current folder?
    ///
    /// nini keeps a session under `sessions/<project directory name>/`
    /// (`nini_core::paths::new_session_path`), and Pi matches the header's
    /// `cwd` (`session-manager.ts#sessionCwdMatches`). Both are checked,
    /// because both are true of nini's own sessions.
    fn in_scope(&self, row: &SessionRow, cwd_name: &str, cwd: Option<&Path>) -> bool {
        if self.scope == SessionScope::All {
            return true;
        }
        if let Some(parent) = row.path.parent().and_then(|p| p.file_name()) {
            if parent.to_string_lossy() == cwd_name {
                return true;
            }
        }
        if let (Some(session_cwd), Some(cwd)) = (row.cwd.as_deref(), cwd) {
            if Path::new(session_cwd) == cwd {
                return true;
            }
        }
        false
    }

    /// The threaded order: forked sessions under the session they came
    /// from (Pi: `buildSessionTree` / `flattenSessionTree`).
    fn threaded<'a>(&self, rows: Vec<&'a SessionRow>) -> Vec<VisibleRow<'a>> {
        threaded_order(&rows)
    }

    /// The right-hand side of a row: the age, the message count and — in
    /// the "all" scope or on demand — the project directory and the path.
    fn description_for(&self, row: &SessionRow) -> String {
        let mut parts = vec![relative_time(row.modified, SystemTime::now())];
        if let Some(count) = row.message_count {
            parts.push(format!("{count} msgs"));
        }
        // Pi shows the project directory in the "all" scope, where the rows
        // come from several projects and the directory is what tells them
        // apart (`session-selector.js:387`).
        if self.scope == SessionScope::All {
            if let Some(cwd) = &row.cwd {
                parts.push(cwd.clone());
            }
        }
        if self.show_path {
            parts.push(row.path.to_string_lossy().to_string());
        }
        parts.join(" · ")
    }
}

/// The threaded order: forked sessions under the session they came from
/// (Pi: `buildSessionTree` / `flattenSessionTree`).
///
/// A free function because two places need it: the picker (which shows
/// the tree) and `picker_order` (which `/resume <n>` uses, so that a
/// number means the same session in both).
fn threaded_order<'a>(rows: &[&'a SessionRow]) -> Vec<VisibleRow<'a>> {
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); rows.len()];
    let mut roots: Vec<usize> = Vec::new();
    // A child is matched to its parent by the parent's session id —
    // the header's `parentSession` holds that id
    // (`nini_session::SessionHeader::new_fork`).
    let ids: Vec<(String, usize)> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r.id.clone().map(|id| (id, i)))
        .collect();
    for (i, row) in rows.iter().enumerate() {
        match row
            .parent_id
            .as_deref()
            .and_then(|p| ids.iter().find(|(id, _)| id == p))
            .map(|(_, idx)| *idx)
        {
            Some(parent) => children[parent].push(i),
            None => roots.push(i),
        }
    }
    let mut out: Vec<VisibleRow<'_>> = Vec::with_capacity(rows.len());
    for (n, root) in roots.iter().enumerate() {
        let is_last = n == roots.len() - 1;
        flatten(*root, 0, is_last, Vec::new(), rows, &children, &mut out);
    }
    out
}

/// Walk a session tree into the flat list the panel shows.
fn flatten<'a>(
    index: usize,
    depth: usize,
    is_last: bool,
    ancestors: Vec<bool>,
    rows: &[&'a SessionRow],
    children: &[Vec<usize>],
    out: &mut Vec<VisibleRow<'a>>,
) {
    out.push(VisibleRow {
        row: rows[index],
        depth,
        is_last,
        ancestors: ancestors.clone(),
    });
    // Pi's `continues`: a node keeps its vertical bar only when it has
    // siblings after it (`session-selector.js#buildTreePrefix`).
    let continues = if depth > 0 { !is_last } else { false };
    let kids = &children[index];
    for (n, child) in kids.iter().enumerate() {
        let mut child_ancestors = ancestors.clone();
        child_ancestors.push(continues);
        flatten(
            *child,
            depth + 1,
            n == kids.len() - 1,
            child_ancestors,
            rows,
            children,
            out,
        );
    }
}

/// Build one row's data out of a session file.
fn row_for(path: &Path) -> SessionRow {
    let meta = nini_session::read_meta_bounded(path, META_BYTES);
    let modified = path
        .metadata()
        .and_then(|m| m.modified())
        .unwrap_or(UNIX_EPOCH);
    SessionRow {
        path: path.to_path_buf(),
        name: meta.as_ref().and_then(|m| m.name.clone()),
        first_message: meta.as_ref().and_then(|m| m.first_user_message.clone()),
        cwd: meta.as_ref().and_then(|m| m.cwd.clone()),
        id: meta.as_ref().and_then(|m| m.id.clone()),
        parent_id: meta.as_ref().and_then(|m| m.parent_session.clone()),
        message_count: meta.as_ref().and_then(|m| m.message_count),
        modified,
    }
}

/// How long ago `then` was, in the compact form the picker shows:
/// `now` / `5m` / `3h` / `2d`.
fn relative_time(then: SystemTime, now: SystemTime) -> String {
    let secs = now.duration_since(then).unwrap_or_default().as_secs();
    if secs < 60 {
        "now".to_string()
    } else if secs < 60 * 60 {
        format!("{}m", secs / 60)
    } else if secs < 24 * 60 * 60 {
        format!("{}h", secs / (60 * 60))
    } else {
        format!("{}d", secs / (24 * 60 * 60))
    }
}

/// Pi's search-box grammar: whitespace-separated fuzzy tokens, and
/// double-quoted phrases that have to appear literally.
///
/// Port of `session-selector-search.js#parseSearchQuery`, minus the `re:`
/// regex mode (nini's TUI crate has no regex dependency — see the module
/// comment).
fn parse_search_query(query: &str) -> Vec<SearchToken> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let mut tokens: Vec<SearchToken> = Vec::new();
    let mut buf = String::new();
    let mut in_quote = false;
    let mut unclosed = false;
    for ch in trimmed.chars() {
        if ch == '"' {
            if in_quote {
                push_token(&mut tokens, &mut buf, true);
                in_quote = false;
            } else {
                push_token(&mut tokens, &mut buf, false);
                in_quote = true;
            }
            continue;
        }
        if !in_quote && ch.is_whitespace() {
            push_token(&mut tokens, &mut buf, false);
            continue;
        }
        buf.push(ch);
    }
    if in_quote {
        unclosed = true;
    }
    if unclosed {
        // Pi falls back to plain whitespace tokenization.
        return trimmed
            .split_whitespace()
            .map(|t| SearchToken::Fuzzy(t.to_string()))
            .collect();
    }
    push_token(&mut tokens, &mut buf, in_quote);
    tokens
}

/// Flush the token buffer into the token list.
fn push_token(tokens: &mut Vec<SearchToken>, buf: &mut String, quoted: bool) {
    let value = buf.trim().to_string();
    buf.clear();
    if value.is_empty() {
        return;
    }
    tokens.push(if quoted {
        SearchToken::Phrase(value)
    } else {
        SearchToken::Fuzzy(value)
    });
}

/// Lowercase and collapse the whitespace, the way Pi's phrase matching
/// does (`session-selector-search.js#normalizeWhitespaceLower`).
fn normalize_whitespace_lower(text: &str) -> String {
    text.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Match a session against the parsed query.
///
/// Port of `session-selector-search.js#matchSession`: every fuzzy token
/// has to match, every phrase has to appear; the score is the sum of the
/// tokens' fuzzy scores and the phrases' positions. Lower is better.
fn match_session(text: &str, tokens: &[SearchToken]) -> Option<f64> {
    if tokens.is_empty() {
        return Some(0.0);
    }
    let mut total = 0.0f64;
    let mut normalized: Option<String> = None;
    for token in tokens {
        match token {
            SearchToken::Phrase(phrase) => {
                let norm = normalized.get_or_insert_with(|| normalize_whitespace_lower(text));
                let phrase = normalize_whitespace_lower(phrase);
                if phrase.is_empty() {
                    continue;
                }
                let idx = norm.find(&phrase)?;
                total += idx as f64 * 0.1;
            }
            SearchToken::Fuzzy(token) => {
                let m = fuzzy_match(token, text);
                if !m.matches {
                    return None;
                }
                total += m.score;
            }
        }
    }
    Some(total)
}

impl SelectorState for SessionSelector {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn state_items(&self) -> Vec<SelectorItem> {
        self.visible_rows()
            .into_iter()
            .map(|v| SelectorItem {
                // The absolute path of the session file: the runtime hands
                // it to `session_load` as a `PathBuf`.
                value: v.row.path.to_string_lossy().to_string(),
                label: format!("{}{}", v.prefix(), v.row.label()),
                description: Some(self.description_for(&v.row)),
                is_current: self.current_session.as_deref() == Some(v.row.path.as_path()),
            })
            .collect()
    }
    fn state_title(&self) -> String {
        if self.renaming.is_some() {
            return "Rename session".to_string();
        }
        let scope = match self.scope {
            SessionScope::Current => "Current Folder",
            SessionScope::All => "All",
        };
        format!("Resume session ({scope})")
    }
    fn state_selected(&self) -> usize {
        self.selected
    }
    fn state_set_selected(&mut self, idx: usize) {
        let len = self.visible_rows().len();
        if idx < len {
            self.selected = idx;
        }
    }
    fn state_on_select(&mut self) -> SelectorOutcome {
        let item = self
            .visible_rows()
            .get(self.selected)
            .map(|v| SelectorItem {
                value: v.row.path.to_string_lossy().to_string(),
                label: format!("{}{}", v.prefix(), v.row.label()),
                description: Some(self.description_for(&v.row)),
                is_current: self.current_session.as_deref() == Some(v.row.path.as_path()),
            });
        match item {
            Some(it) => {
                self.result = Some(it.value.clone());
                SelectorOutcome::Picked(it)
            }
            None => SelectorOutcome::Cancelled,
        }
    }
    /// Pi's own filter: the search text is the session's id, name, first
    /// message and cwd, matched token by token
    /// (`session-selector-search.js#matchSession`).
    fn state_filter(&self, query: &str) -> Vec<usize> {
        let rows = self.visible_rows();
        let tokens = parse_search_query(query);
        if tokens.is_empty() {
            return (0..rows.len()).collect();
        }
        let mut scored: Vec<(usize, f64)> = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            if let Some(score) = match_session(&row.row.search_text(), &tokens) {
                scored.push((i, score));
            }
        }
        if self.sort == SessionSort::Recent {
            // "Recent" only filters — the order stays the loaded one
            // (`session-selector-search.js#filterAndSortSessions`).
            return scored.into_iter().map(|(i, _)| i).collect();
        }
        // "Relevance" (and "threaded" with a query, which Pi sorts by
        // score too) orders by the score, lowest first.
        scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().map(|(i, _)| i).collect()
    }
    /// What the panel says when there is nothing to show.
    ///
    /// Pi's wording (`session-selector.js:404-419`), which differs by
    /// scope and by the named filter — the difference is the whole point:
    /// "no sessions" and "no *named* sessions" need different advice.
    fn state_empty_message(&self) -> Option<String> {
        Some(match (self.name_filter, self.scope) {
            (SessionNameFilter::Named, SessionScope::All) => {
                "No named sessions found. Press Ctrl+N to show all.".to_string()
            }
            (SessionNameFilter::Named, SessionScope::Current) => {
                "No named sessions in current folder. Press Ctrl+N to show all, or Tab to view all."
                    .to_string()
            }
            (SessionNameFilter::All, SessionScope::All) => "No sessions found".to_string(),
            (SessionNameFilter::All, SessionScope::Current) => {
                "No sessions in current folder. Press Tab to view all.".to_string()
            }
        })
    }
    /// While renaming, the input row shows the name being typed.
    fn state_input_text(&self) -> Option<String> {
        if self.renaming.is_some() {
            Some(self.rename_input.clone())
        } else {
            None
        }
    }
}
