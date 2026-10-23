//! `write` tool: create or overwrite a file atomically.

use async_trait::async_trait;
use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolSpec};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::fs;

#[derive(Debug, Deserialize)]
struct WriteArgs {
    path: String,
    content: String,
}

#[derive(Debug, Default)]
pub struct WriteTool;

impl WriteTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "write"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write".to_string(),
            // v0.10.8 (T1): 与 Pi 0.85.1 的契约逐字对齐
            // （references/pi-0.85.1/tool-contracts.json，由 scripts/pi-tool-dump.mjs
            //  执行 Pi 自己的代码产出）。
            description: "Write content to a file. Creates the file if it doesn't exist, \
                          overwrites if it does. Automatically creates parent directories."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to write (relative or absolute)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write to the file"
                    }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        }
    }

    fn system_prompt_contribution(&self) -> Option<nini_core::tool::ToolSystemPrompt> {
        Some(nini_core::tool::ToolSystemPrompt {
            // v0.10.8 (T1): 与 Pi 0.85.1 的契约逐字对齐
            // （references/pi-0.85.1/tool-contracts.json，由 scripts/pi-tool-dump.mjs
            //  执行 Pi 自己的代码产出）。
            snippet: "Create or overwrite files".to_string(),
            guidelines: vec!["Use write only for new files or complete rewrites.".to_string()],
        })
    }

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let parsed: WriteArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArgs(e.to_string()))?;

        let path = if Path::new(&parsed.path).is_absolute() {
            PathBuf::from(&parsed.path)
        } else {
            ctx.cwd.join(&parsed.path)
        };

        // v0.10 (G2): serialise against other mutations of the same path —
        // see the long note in `edit.rs`. `write` is a whole-file replace, so
        // two concurrent writes to one file are last-writer-wins by
        // definition; the queue makes the *order* deterministic relative to
        // the tool batch instead of racing the filesystem, and stops a
        // `write` from interleaving with an `edit` of the same file.
        crate::mutation_queue::with_file_mutation_queue(&path, || async {
            // Ensure parent dir
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent).await?;
                }
            }

            // Atomic write: write to <path>.tmp.<rand>, then rename
            let parent = path.parent().unwrap_or_else(|| Path::new("."));
            let file_name = path
                .file_name()
                .ok_or_else(|| ToolError::InvalidArgs("path has no file name".to_string()))?;
            // v0.10 (G2): the name was `.{file}.nini-tmp.{pid}` — one per
            // process per file, so two concurrent writes to that file truncated
            // and rewrote the *same* path. `unique_temp_name` adds a monotonic
            // counter.
            let tmp = parent.join(crate::mutation_queue::unique_temp_name(
                &file_name.to_string_lossy(),
            ));
            fs::write(&tmp, parsed.content.as_bytes()).await?;
            if let Err(e) = fs::rename(&tmp, &path).await {
                // Best-effort cleanup
                let _ = fs::remove_file(&tmp).await;
                return Err(ToolError::Io(e.to_string()));
            }

            Ok(ToolOutput {
                content: format!("wrote {} bytes to {}", parsed.content.len(), path.display()),
                is_error: false,
                details: Some(json!({
                    "path": path.display().to_string(),
                    "bytes": parsed.content.len(),
                })),
                duration_ms: 0,
                terminate: false,
            })
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn write_creates_new_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(r"new.txt");
        let tool = WriteTool::new();
        let out = tool
            .execute(
                json!({"path": path.to_str().unwrap(), "content": "hello\n"}),
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(!out.is_error);
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "hello\n");
    }

    #[tokio::test]
    async fn write_overwrites_existing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(r"a.txt");
        std::fs::write(&path, "old").unwrap();

        let tool = WriteTool::new();
        let out = tool
            .execute(
                json!({"path": path.to_str().unwrap(), "content": "new"}),
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(!out.is_error);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }

    #[tokio::test]
    async fn write_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(r"sub/dir/file.txt");
        let tool = WriteTool::new();
        let out = tool
            .execute(
                json!({"path": path.to_str().unwrap(), "content": "x"}),
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(path.exists());
    }

    #[tokio::test]
    async fn write_rejects_missing_path() {
        let tool = WriteTool::new();
        // Missing `path` field => deserialization fails => InvalidArgs
        let err = tool
            .execute(json!({"content": "x"}), ToolContext::default())
            .await;
        assert!(matches!(err, Err(ToolError::InvalidArgs(_))));
    }
}
