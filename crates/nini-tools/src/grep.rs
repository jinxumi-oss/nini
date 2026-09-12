//! `grep` tool: regex content search across files.
//!
//! Uses Rust's `regex` crate for pattern matching and `ignore` crate for
//! `.gitignore`-aware file walking. Output is `file:line:content` per line.

use async_trait::async_trait;
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use regex::Regex;
use serde::Deserialize;
use serde_json::{ json, Value };
use std::path::PathBuf;
use nini_core::tool::{Tool, ToolContext, ToolOutput, ToolError, ToolSpec};

#[derive(Debug, Deserialize)]
struct GrepArgs {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    include: Option<String>,
}

#[derive(Debug, Default)]
pub struct GrepTool;

impl GrepTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "grep".to_string(),
            description: "Regex search across files in a directory. Honors .gitignore. \
                          Output is `file:line:content` per match."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern (Rust regex syntax)."
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory or file to search in. Defaults to cwd."
                    },
                    "include": {
                        "type": "string",
                        "description": "Optional glob to filter files (e.g., `*.rs`)."
                    }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let parsed: GrepArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArgs(e.to_string()))?;

        let regex =
            Regex::new(&parsed.pattern).map_err(|e| ToolError::InvalidArgs(format!("invalid regex: {e}")))?;

        let path: PathBuf = parsed
            .path
            .as_deref()
            .map(|p| {
                if std::path::Path::new(p).is_absolute() {
                    PathBuf::from(p)
                } else {
                    ctx.cwd.join(p)
                }
            })
            .unwrap_or_else(|| ctx.cwd.clone());

        // Build walker with optional glob override
        let mut walker = WalkBuilder::new(&path);
        walker.follow_links(false);
        if let Some(ref glob) = parsed.include {
            let mut ob = OverrideBuilder::new(&path);
            // `!*.ext` excludes, `*.ext` includes — invert: user wants only matching
            if let Err(e) = ob.add(&format!("!{glob}")) {
                return Err(ToolError::InvalidArgs(format!("invalid include glob: {e}")));
            }
            walker.overrides(ob.build().expect("valid override"));
        }

        let mut results: Vec<String> = Vec::new();
        let mut files_scanned = 0usize;

        for entry in walker.build() {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            files_scanned += 1;
            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            for (idx, line) in content.lines().enumerate() {
                if regex.is_match(line) {
                    results.push(format!("{}:{}:{}", entry.path().display(), idx + 1, line));
                }
                if results.len() >= ctx.max_output_lines {
                    break;
                }
            }
            if results.len() >= ctx.max_output_lines {
                break;
            }
        }

        let mut output = results.join("\n");
        if output.is_empty() {
            output = format!("no matches for /{}/", parsed.pattern);
        } else if results.len() >= ctx.max_output_lines {
            output.push_str(&format!(
                "\n[... output truncated at {} lines ...]",
                ctx.max_output_lines
            ));
        }

        Ok(ToolOutput {
            content: output,
            is_error: false,
            details: Some(json!({
                "files_scanned": files_scanned,
                "matches": results.len(),
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[tokio::test]
    async fn grep_finds_matching_files() {
        let dir = tempdir().unwrap();
        let mut a = std::fs::File::create(dir.path().join("a.txt")).unwrap();
        writeln!(a, "alpha line 1\nbeta line 2").unwrap();
        let mut b = std::fs::File::create(dir.path().join("b.txt")).unwrap();
        writeln!(b, "alpha line\nzzz").unwrap();

        let tool = GrepTool::new();
        let out = tool
            .execute(json!({"pattern": "alpha", "path": dir.path().to_str().unwrap()}), ToolContext::default())
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("a.txt"), "got: {}", out.content);
        assert!(out.content.contains("b.txt"), "got: {}", out.content);
        assert_eq!(out.content.matches('\n').count() + 1, 2);
    }

    #[tokio::test]
    async fn grep_invalid_regex_reports_error() {
        let tool = GrepTool::new();
        let err = tool
            .execute(json!({"pattern": "("}), ToolContext::default())
            .await;
        assert!(matches!(err, Err(ToolError::InvalidArgs(_))));
    }

    #[tokio::test]
    async fn grep_no_matches_returns_message() {
        let dir = tempdir().unwrap();
        let tool = GrepTool::new();
        let out = tool
            .execute(json!({"pattern": "nonexistent", "path": dir.path().to_str().unwrap()}), ToolContext::default())
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("no matches"));
    }
}