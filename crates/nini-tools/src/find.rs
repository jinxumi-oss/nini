//! `find` tool: file/directory discovery by glob pattern.

use async_trait::async_trait;
use ignore::WalkBuilder;
use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolSpec};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct FindArgs {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Default)]
pub struct FindTool;

impl FindTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for FindTool {
    fn name(&self) -> &'static str {
        "find"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "find".to_string(),
            description: "Find files by glob pattern (e.g., `*.rs`, `**/*.toml`). \
                          Honors .gitignore."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern to match file names."
                    },
                    "path": {
                        "type": "string",
                        "description": "Root directory. Defaults to cwd."
                    },
                    "limit": {
                        "type": "number",
                        "description": "Maximum number of results. Default 1000."
                    }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let parsed: FindArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArgs(e.to_string()))?;

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

        let limit = parsed.limit.unwrap_or(1000);

        // Build a regex from glob: convert `*` → `.*`, `?` → `.`, escape other regex chars
        let pattern_regex = glob_to_regex(&parsed.pattern)
            .map_err(|e| ToolError::InvalidArgs(format!("invalid pattern: {e}")))?;
        let matcher = regex::Regex::new(&pattern_regex)
            .map_err(|e| ToolError::InvalidArgs(format!("invalid pattern: {e}")))?;

        let mut results: Vec<String> = Vec::new();
        let walker = WalkBuilder::new(&path).follow_links(false).build();
        for entry in walker.flatten() {
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let name = entry
                .path()
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if matcher.is_match(name) || matcher.is_match(&entry.path().to_string_lossy()) {
                results.push(entry.path().display().to_string());
                if results.len() >= limit {
                    break;
                }
            }
        }

        let mut output = results.join("\n");
        if output.is_empty() {
            output = format!("no files matching `{}`", parsed.pattern);
        } else if results.len() >= limit {
            output.push_str(&format!("\n[... truncated at {limit} results]"));
        }

        Ok(ToolOutput {
            content: output,
            is_error: false,
            details: Some(json!({
                "matches": results.len(),
                "pattern": parsed.pattern,
            })),
        })
    }
}

/// Convert a glob pattern to a regex string.
/// Supports `*`, `**`, `?`. Other regex metacharacters are escaped.
fn glob_to_regex(glob: &str) -> Result<String, String> {
    let mut out = String::from("^");
    let mut chars = glob.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    out.push_str(".*");
                } else {
                    out.push_str("[^/]*");
                }
            }
            '?' => out.push('.'),
            '.' | '(' | ')' | '+' | '|' | '^' | '$' | '{' | '}' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '[' => {
                out.push('[');
                while let Some(&nc) = chars.peek() {
                    chars.next();
                    out.push(nc);
                    if nc == ']' {
                        break;
                    }
                }
            }
            other => out.push(other),
        }
    }
    out.push('$');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn find_returns_matching_files() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "").unwrap();
        fs::write(dir.path().join("b.rs"), "").unwrap();
        fs::write(dir.path().join("c.toml"), "").unwrap();

        let tool = FindTool::new();
        let out = tool
            .execute(
                json!({"pattern": "*.rs", "path": dir.path().to_str().unwrap()}),
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("b.rs"));
        assert!(!out.content.contains("a.txt"));
    }

    #[tokio::test]
    async fn find_returns_empty_message_when_no_matches() {
        let dir = tempdir().unwrap();
        let tool = FindTool::new();
        let out = tool
            .execute(
                json!({"pattern": "*.nonexistent", "path": dir.path().to_str().unwrap()}),
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("no files"));
    }

    #[test]
    fn glob_to_regex_basic() {
        assert_eq!(glob_to_regex("*.rs").unwrap(), "^[^/]*\\.rs$");
        assert_eq!(glob_to_regex("**/*.toml").unwrap(), "^.*/[^/]*\\.toml$");
        assert_eq!(glob_to_regex("file?").unwrap(), "^file.$");
    }
}
