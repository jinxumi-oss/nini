//! `read` tool: read a file (with image/http passthrough deferred to Phase 3).

use async_trait::async_trait;
use nini_core::tool::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use tokio::fs;

#[derive(Debug, Deserialize)]
struct ReadArgs {
    path: String,
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Debug, Default)]
pub struct ReadTool;

impl ReadTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".to_string(),
            description: "Read a file's contents into context. Use `offset` + `limit` \
                          for large files (line numbers are 1-indexed)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or cwd-relative file path."
                    },
                    "offset": {
                        "type": "number",
                        "description": "Line offset to start reading from (0-indexed). Optional."
                    },
                    "limit": {
                        "type": "number",
                        "description": "Maximum number of lines to read. Optional."
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let parsed: ReadArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArgs(e.to_string()))?;

        let path = if Path::new(&parsed.path).is_absolute() {
            Path::new(&parsed.path).to_path_buf()
        } else {
            ctx.cwd.join(&parsed.path)
        };

        let content = fs::read_to_string(&path)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => {
                    ToolError::Io(format!("file not found: {}", path.display()))
                }
                std::io::ErrorKind::PermissionDenied => {
                    ToolError::PermissionDenied(format!("cannot read: {}", path.display()))
                }
                _ => ToolError::Io(e.to_string()),
            })?;

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();
        let offset = parsed.offset.unwrap_or(0) as usize;
        let limit = parsed.limit.map(|n| n as usize).unwrap_or(usize::MAX);
        let end = (offset + limit).min(total_lines);
        let start = offset.min(total_lines);

        let selected = if start >= end {
            String::new()
        } else {
            lines[start..end].join("\n")
        };

        let details = json!({
            "path": path.display().to_string(),
            "total_lines": total_lines,
            "offset": offset,
            "limit": if limit == usize::MAX { None } else { Some(limit) },
        });

        Ok(ToolOutput {
            content: selected,
            is_error: false,
            details: Some(details),
        })
    }

    fn system_prompt_contribution(&self) -> Option<nini_core::tool::ToolSystemPrompt> {
        Some(nini_core::tool::ToolSystemPrompt {
            snippet: "Read a file's contents into your context.".into(),
            guidelines: vec![
                "Always read a file before editing it — never edit blind.".into(),
                "For files >~1MB, use `offset` + `limit` to read in chunks.".into(),
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use tempfile::NamedTempFile;

    #[test]
    fn read_spec_is_concise_and_nonoverlapping() {
        // v0.8 regression guard: snippet + description must not duplicate
        // each other, and snippet must be short (Pi-style: ~10 words).
        let tool = ReadTool::new();
        let contrib = tool.system_prompt_contribution().unwrap();
        let word_count = contrib.snippet.split_whitespace().count();
        assert!(
            word_count <= 10,
            "read snippet too long: {} words — {}",
            word_count, contrib.snippet,
        );
        let spec = tool.spec();
        assert!(
            !contrib.snippet.is_empty(),
            "read snippet must not be empty",
        );
        // Snippet must be a distinct perspective from description —
        // simplest sanity check: neither should be a prefix of the other.
        let s_lower = contrib.snippet.to_lowercase();
        let d_lower = spec.description.to_lowercase();
        assert!(
            !d_lower.starts_with(&s_lower),
            "read description should not start with the snippet's text",
        );
    }

    #[tokio::test]
    async fn read_full_file() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "line 1\nline 2\nline 3").unwrap();
        let path = f.path().to_str().unwrap();

        let tool = ReadTool::new();
        let out = tool
            .execute(json!({"path": path}), ToolContext::default())
            .await
            .unwrap();
        assert!(out.content.contains("line 1"));
        assert!(out.content.contains("line 3"));
    }

    #[tokio::test]
    async fn read_with_offset_and_limit() {
        let mut f = NamedTempFile::new().unwrap();
        for i in 0..10 {
            writeln!(f, "line {i}").unwrap();
        }
        let path = f.path().to_str().unwrap();

        let tool = ReadTool::new();
        let out = tool
            .execute(
                json!({"path": path, "offset": 3, "limit": 2}),
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(out.content, "line 3\nline 4");
    }

    #[tokio::test]
    async fn read_missing_file_returns_error() {
        let tool = ReadTool::new();
        let err = tool
            .execute(
                json!({"path": "/nonexistent/path/abc.txt"}),
                ToolContext::default(),
            )
            .await;
        assert!(matches!(err, Err(ToolError::Io(_))));
    }
}
