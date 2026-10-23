//! `find` tool: file/directory discovery by glob pattern.

use async_trait::async_trait;
use ignore::WalkBuilder;
use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolSpec, ToolSystemPrompt};
use serde::Deserialize;
use serde_json::{json, Value};
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
            // v0.10.8 (T1): 与 Pi 0.85.1 的契约逐字对齐
            description: "Search for files by glob pattern. Returns matching file paths \
                          relative to the search directory. Respects .gitignore. Output is \
                          truncated to 1000 results or 50KB (whichever is hit first)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern to match files, e.g. '*.ts', '**/*.json', or 'src/**/*.spec.ts'"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in (default: current directory)"
                    },
                    "limit": {
                        "type": "number",
                        "description": "Maximum number of results (default: 1000)"
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
        } else {
            // v0.10.8 (T4): 统一截断（Pi 的 tools/truncate.js：行数与字节两个
            // 上限，先到者胜）。此前 nini 只有「条目数到顶」一种提示，
            // 字节上限根本不管——于是大结果集会灌进上下文。
            let t =
                crate::truncate::truncate_head(&output, ctx.max_output_lines, ctx.max_output_bytes);
            output = t.content;
            if t.truncated {
                output.push_str(&format!(
                    "\n[... truncated at {limit} results, {} of the output kept]",
                    crate::truncate::format_size(t.output_bytes)
                ));
            }
        }

        Ok(ToolOutput {
            content: output,
            is_error: false,
            details: Some(json!({
                "matches": results.len(),
                "pattern": parsed.pattern,
            })),
            duration_ms: 0,
            terminate: false,
        })
    }

    fn system_prompt_contribution(&self) -> Option<ToolSystemPrompt> {
        Some(ToolSystemPrompt {
            // v0.10.8 (T1): Pi 0.85.1 的 find 没有 guidelines
            snippet: "Find files by glob pattern (respects .gitignore)".into(),
            guidelines: vec![],
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
    fn find_spec_is_concise_and_nonoverlapping() {
        // v0.8 regression guard: snippet + description must not duplicate
        // each other, and snippet must be short (Pi-style: ~10 words).
        let tool = FindTool::new();
        let contrib = tool.system_prompt_contribution().unwrap();
        let word_count = contrib.snippet.split_whitespace().count();
        assert!(
            word_count <= 10,
            "find snippet too long: {} words — {}",
            word_count,
            contrib.snippet,
        );
        let spec = tool.spec();
        assert!(
            !contrib.snippet.is_empty(),
            "find snippet must not be empty",
        );
        // Snippet must be a distinct perspective from description —
        // simplest sanity check: neither should be a prefix of the other.
        let s_lower = contrib.snippet.to_lowercase();
        let d_lower = spec.description.to_lowercase();
        assert!(
            !d_lower.starts_with(&s_lower),
            "find description should not start with the snippet's text",
        );
    }

    #[test]
    fn glob_to_regex_basic() {
        assert_eq!(glob_to_regex("*.rs").unwrap(), "^[^/]*\\.rs$");
        assert_eq!(glob_to_regex("**/*.toml").unwrap(), "^.*/[^/]*\\.toml$");
        assert_eq!(glob_to_regex("file?").unwrap(), "^file.$");
    }
}
