//! `write` tool: create or overwrite a file atomically.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{ json, Value };
use std::path::{ Path, PathBuf };
use tokio::fs;
use nini_core::tool::{Tool, ToolContext, ToolOutput, ToolError, ToolSpec};

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
            description: "Write a file atomically (temp file + rename). Creates parent \
                          directories. Overwrites if the file exists."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or cwd-relative path to write to."
                    },
                    "content": {
                        "type": "string",
                        "description": "Full file content to write."
                    }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let parsed: WriteArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArgs(e.to_string()))?;

        let path = if Path::new(&parsed.path).is_absolute() {
            PathBuf::from(&parsed.path)
        } else {
            ctx.cwd.join(&parsed.path)
        };

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
        let tmp = parent.join(format!(
            ".{}.nini-tmp.{}",
            file_name.to_string_lossy(),
            std::process::id()
        ));
        fs::write(&tmp, parsed.content.as_bytes()).await?;
        if let Err(e) = fs::rename(&tmp, &path).await {
            // Best-effort cleanup
            let _ = fs::remove_file(&tmp).await;
            return Err(ToolError::Io(e.to_string()));
        }

        Ok(ToolOutput {
            content: format!(
                "wrote {} bytes to {}",
                parsed.content.len(),
                path.display()
            ),
            is_error: false,
            details: Some(json!({
                "path": path.display().to_string(),
                "bytes": parsed.content.len(),
            })),
        })
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
            .execute(json!({"path": path.to_str().unwrap(), "content": "hello\n"}), ToolContext::default())
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
            .execute(json!({"path": path.to_str().unwrap(), "content": "new"}), ToolContext::default())
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
            .execute(json!({"path": path.to_str().unwrap(), "content": "x"}), ToolContext::default())
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