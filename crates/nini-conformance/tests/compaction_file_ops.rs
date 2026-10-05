//! Oracle: compaction summaries must record which files the run touched.
//!
//! Verified against Pi 0.84.2's `compaction/utils.ts` (restored from
//! `dist/*.js.map` `sourcesContent`):
//!
//! ```ts
//! // utils.ts:24-52 — extractFileOpsFromMessage
//! if (message.role !== "assistant") return;
//! for (const block of message.content) {
//!   if (block.type !== "toolCall") continue;
//!   const path = block.arguments?.path; if (!path) continue;
//!   switch (block.name) {
//!     case "read":  fileOps.read.add(path);   break;
//!     case "write": fileOps.written.add(path); break;
//!     case "edit":  fileOps.edited.add(path);  break;
//!   }
//! }
//!
//! // utils.ts:54-58 — computeFileLists
//! const modified   = new Set([...fileOps.edited, ...fileOps.written]);
//! const readOnly   = [...fileOps.read].filter(f => !modified.has(f)).sort();
//! const modifiedF  = [...modified].sort();
//!
//! // utils.ts:62-74 — formatFileOperations
//! // <read-files>…</read-files> / <modified-files>…</modified-files>
//! // returns "" when both lists are empty
//! ```
//!
//! nini shipped a `file_ops` accumulator in `generate_local_summary` that
//! was declared `Vec::new()` and never filled, so the render branch below it
//! was dead and every summary silently omitted the file inventory. The
//! extractor behind it was worse than unwired: it read `Role::Tool` message
//! text, split it on whitespace and regex-matched path-shaped tokens, and had
//! no tool-name dispatch at all — so `modified_files` could never be
//! non-empty, and any path quoted inside a file's contents looked like a read.

use nini_core::compaction::generate_local_summary;
use nini_core::provider::{ContentBlock, Message, Role};

fn msg(role: Role, blocks: Vec<ContentBlock>) -> Message {
    Message {
        role,
        content: blocks,
        timestamp: 0,
    }
}

fn tool_call(name: &str, path: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: format!("id-{name}-{path}"),
        name: name.to_string(),
        input: serde_json::json!({ "path": path }),
    }
}

fn entry(id: &str, m: Message) -> nini_core::Entry {
    nini_core::Entry::message(id, None, 1, nini_core::AgentMessage::from(m))
}

#[test]
fn read_write_and_edit_are_classified_by_tool_name() {
    let entries = vec![entry(
        "a1",
        msg(
            Role::Assistant,
            vec![
                tool_call("read", "src/alpha.rs"),
                tool_call("read", "src/beta.rs"),
                tool_call("write", "src/gamma.rs"),
                tool_call("edit", "src/delta.rs"),
            ],
        ),
    )];
    let (read, modified) = nini_core::branch_summary::extract_file_operations(&entries);
    assert_eq!(
        read,
        vec!["src/alpha.rs".to_string(), "src/beta.rs".to_string()],
        "read-only paths, sorted"
    );
    assert_eq!(
        modified,
        vec!["src/delta.rs".to_string(), "src/gamma.rs".to_string()],
        "edited ∪ written, sorted"
    );
}

#[test]
fn a_path_that_is_both_read_and_modified_is_reported_only_as_modified() {
    // Pi `utils.ts:56`: `readOnly = [...read].filter(f => !modified.has(f))`.
    // Listing it under read-files would tell the summarizer the file was
    // left untouched.
    let entries = vec![entry(
        "a1",
        msg(
            Role::Assistant,
            vec![
                tool_call("read", "src/touched.rs"),
                tool_call("edit", "src/touched.rs"),
            ],
        ),
    )];
    let (read, modified) = nini_core::branch_summary::extract_file_operations(&entries);
    assert!(
        read.is_empty(),
        "modified file must not appear as read-only"
    );
    assert_eq!(modified, vec!["src/touched.rs".to_string()]);
}

#[test]
fn paths_quoted_inside_tool_results_are_not_operations() {
    // The pre-v0.10 scraper read `Role::Tool` text, so this would have been
    // reported as files the agent read. It is file *content*.
    let entries = vec![
        entry(
            "a1",
            msg(Role::Assistant, vec![tool_call("read", "src/real.rs")]),
        ),
        entry(
            "t1",
            msg(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "id-read-src/real.rs".into(),
                    content: "here is a list: /etc/passwd ./not-read.txt ../nope.md".into(),
                    is_error: false,
                }],
            ),
        ),
    ];
    let (read, modified) = nini_core::branch_summary::extract_file_operations(&entries);
    assert_eq!(read, vec!["src/real.rs".to_string()]);
    assert!(modified.is_empty());
}

#[test]
fn non_path_tools_and_missing_path_arguments_are_ignored() {
    let entries = vec![entry(
        "a1",
        msg(
            Role::Assistant,
            vec![
                ContentBlock::ToolUse {
                    id: "1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({ "command": "cat /etc/shadow" }),
                },
                ContentBlock::ToolUse {
                    id: "2".into(),
                    name: "read".into(),
                    input: serde_json::json!({ "offset": 1 }),
                },
                ContentBlock::Text {
                    text: "thinking about /tmp".into(),
                },
            ],
        ),
    )];
    let (read, modified) = nini_core::branch_summary::extract_file_operations(&entries);
    assert!(read.is_empty(), "got {read:?}");
    assert!(modified.is_empty(), "got {modified:?}");
}

#[test]
fn summary_renders_pi_sections_and_omits_them_when_empty() {
    let entries = vec![entry(
        "a1",
        msg(
            Role::Assistant,
            vec![tool_call("read", "src/a.rs"), tool_call("edit", "src/b.rs")],
        ),
    )];
    let summary = generate_local_summary(&entries);
    assert!(
        summary.contains("<read-files>\nsrc/a.rs\n</read-files>"),
        "missing Pi read-files section:\n{summary}"
    );
    assert!(
        summary.contains("<modified-files>\nsrc/b.rs\n</modified-files>"),
        "missing Pi modified-files section:\n{summary}"
    );

    // Nothing touched ⇒ nothing rendered. The old code emitted both tags
    // unconditionally, so "no operations" was indistinguishable from
    // "the summarizer never looked".
    let empty = generate_local_summary(&[entry(
        "a1",
        msg(Role::User, vec![ContentBlock::Text { text: "hi".into() }]),
    )]);
    assert!(
        !empty.contains("<read-files>") && !empty.contains("<modified-files>"),
        "empty inventory must render no tags:\n{empty}"
    );
}

#[test]
fn format_returns_empty_string_for_no_operations() {
    assert_eq!(
        nini_core::branch_summary::format_file_operations(&[], &[]),
        ""
    );
    let one = vec!["x.rs".to_string()];
    let out = nini_core::branch_summary::format_file_operations(&one, &[]);
    assert!(out.contains("<read-files>") && !out.contains("<modified-files>"));
    assert!(
        out.starts_with("\n\n"),
        "Pi prefixes with a blank line: {out:?}"
    );
}
