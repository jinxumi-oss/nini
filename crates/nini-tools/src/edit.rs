//! `edit` tool: surgically replace a string in a file.
//!
//! Spec mirrors `packages/coding-agent/src/core/tools/edit.ts`:
//! - `oldText` must match exactly once (errors if not found or matches many)
//! - Replaces the first occurrence with `newText`
//! - Atomic write via temp file + rename

use async_trait::async_trait;
use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolSpec};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use crate::diff;
use tokio::fs;

#[derive(Debug, Deserialize)]
struct EditArgs {
    path: String,
    old_text: String,
    new_text: String,
}

#[derive(Debug, Default)]
pub struct EditTool;

impl EditTool {
    pub fn new() -> Self {
        Self
    }

    /// Apply the edit synchronously (no I/O).
    fn apply(content: &str, old: &str, new: &str) -> Result<(String, usize), String> {
        if old.is_empty() {
            return Err("old_text must not be empty".to_string());
        }
        let occurrences = content.matches(old).count();
        match occurrences {
            0 => Err(format!(
                "old_text not found in file (searched {} chars)",
                content.len()
            )),
            1 => Ok((content.replacen(old, new, 1), 1)),
            n => Err(format!(
                "old_text matches {n} locations; must match exactly 1"
            )),
        }
    }
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".to_string(),
            description: "Replace `old_text` with `new_text` in a file. old_text must match \
                          exactly once. Atomic write via temp file + rename."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or cwd-relative file path."
                    },
                    "old_text": {
                        "type": "string",
                        "description": "Exact text to find. Must match exactly once."
                    },
                    "new_text": {
                        "type": "string",
                        "description": "Replacement text."
                    }
                },
                "required": ["path", "old_text", "new_text"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let parsed: EditArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArgs(e.to_string()))?;

        let path = if Path::new(&parsed.path).is_absolute() {
            PathBuf::from(&parsed.path)
        } else {
            ctx.cwd.join(&parsed.path)
        };

        let content = fs::read_to_string(&path)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => {
                    ToolError::Io(format!("file not found: {}", path.display()))
                }
                _ => ToolError::Io(e.to_string()),
            })?;

        let (new_content, replacements) = Self::apply(&content, &parsed.old_text, &parsed.new_text)
            .map_err(ToolError::InvalidArgs)?;

        // Atomic write
        let parent = path.parent().unwrap_or(Path::new("."));
        let file_name = path.file_name().unwrap();
        let tmp = parent.join(format!(
            ".{}.nini-tmp.{}",
            file_name.to_string_lossy(),
            std::process::id()
        ));
        fs::write(&tmp, new_content.as_bytes()).await?;
        if let Err(e) = fs::rename(&tmp, &path).await {
            let _ = fs::remove_file(&tmp).await;
            return Err(ToolError::Io(e.to_string()));
        }

        // Build a unified diff for the TUI to render with +/- coloring.
        // Context of 3 matches `diff -u` default behavior.
        let diff_text = diff::render_unified(&content, &new_content, 3);
        let (adds, dels) = diff::diff_summary(&content, &new_content);

        Ok(ToolOutput {
            content: format!(
                "replaced {} occurrence(s) in {}\n\n{}",
                replacements,
                path.display(),
                diff_text
            ),
            is_error: false,
            details: Some(json!({
                "path": path.display().to_string(),
                "replacements": replacements,
                "additions": adds,
                "deletions": dels,
                "diff": diff_text,
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_file(content: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, content).unwrap();
        (dir, path)
    }

    #[tokio::test]
    async fn edit_replaces_single_occurrence() {
        let (_dir, path) = make_file("hello world\n");
        let tool = EditTool::new();
        let out = tool
            .execute(
                json!({"path": path.to_str().unwrap(), "old_text": "world", "new_text": "rust"}),
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(!out.is_error, "got: {:?}", out);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello rust\n");
    }

    #[tokio::test]
    async fn edit_rejects_zero_matches() {
        let (_dir, path) = make_file("hello world\n");
        let tool = EditTool::new();
        let err = tool
            .execute(
                json!({"path": path.to_str().unwrap(), "old_text": "missing", "new_text": "x"}),
                ToolContext::default(),
            )
            .await;
        assert!(matches!(err, Err(ToolError::InvalidArgs(_))));
    }

    #[tokio::test]
    async fn edit_rejects_multiple_matches() {
        let (_dir, path) = make_file("foo foo foo\n");
        let tool = EditTool::new();
        let err = tool
            .execute(
                json!({"path": path.to_str().unwrap(), "old_text": "foo", "new_text": "bar"}),
                ToolContext::default(),
            )
            .await;
        assert!(matches!(err, Err(ToolError::InvalidArgs(_))));
    }

    #[tokio::test]
    async fn edit_rejects_empty_old_text() {
        let (_dir, path) = make_file("hello\n");
        let tool = EditTool::new();
        let err = tool
            .execute(
                json!({"path": path.to_str().unwrap(), "old_text": "", "new_text": "x"}),
                ToolContext::default(),
            )
            .await;
        assert!(matches!(err, Err(ToolError::InvalidArgs(_))));
    }

    #[test]
    fn apply_logic() {
        let (new, n) = EditTool::apply("abc xyz", "abc", "x").unwrap();
        assert_eq!(new, "x xyz");
        assert_eq!(n, 1);
        assert!(EditTool::apply("abc abc", "abc", "x").is_err()); // 2 matches
        assert!(EditTool::apply("xyz", "abc", "x").is_err()); // 0 matches
        assert!(EditTool::apply("any", "", "x").is_err()); // empty old
    }
}

#[cfg(test)]
mod diff_integration_tests {
    use super::*;
    use crate::diff::{diff_summary, render_unified};

    #[test]
    fn edit_returns_diff_in_details() {
        let (adds, dels) = diff_summary("line1\nline2\nline3\n", "line1\nmodified\nline3\n");
        assert_eq!(adds, 1);
        assert_eq!(dels, 1);
    }

    #[test]
    fn edit_diff_renders_unified_format() {
        let d = render_unified("a\nb\nc\n", "a\nB\nc\n", 1);
        // Should contain a `-b`, `+B`, and context lines.
        assert!(d.lines().any(|l| l.starts_with("-b")));
        assert!(d.lines().any(|l| l.starts_with("+B")));
        assert!(d.lines().any(|l| l == " a"));
        assert!(d.lines().any(|l| l == " c"));
    }
}
