//! `grep` tool: regex content search across files.
//!
//! Uses Rust's `regex` crate for pattern matching and `ignore` crate for
//! `.gitignore`-aware file walking. Output is `file:line:content` per line.

use async_trait::async_trait;
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolSpec, ToolSystemPrompt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

/// Pi 0.85.1 的 grep 参数集（见 references/pi-0.85.1/tool-contracts.json）。
///
/// v0.10.8 (T1)：此前 nini 只有 pattern/path/include 三个参数，
/// 而 Pi 有七个——**少 advertise 四个能力，模型就永远不会用它们**。
/// `include` 也改名为 Pi 的 `glob`（模型按 schema 学名字）。
#[derive(Debug, Deserialize)]
struct GrepArgs {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    ignore_case: Option<bool>,
    #[serde(default)]
    literal: Option<bool>,
    #[serde(default)]
    context: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
}

/// Pi 的默认匹配数上限。
const DEFAULT_LIMIT: usize = 100;
/// Pi 的单行截断宽度（`GREP_MAX_LINE_LENGTH`）。
const GREP_MAX_LINE_LENGTH: usize = 500;

/// 相对 cwd 的显示路径（Pi 的 formatPath）。
fn relative_display(path: &std::path::Path, cwd: &std::path::Path) -> String {
    match path.strip_prefix(cwd) {
        Ok(rel) => rel.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

/// 截断一行到 Pi 的 `GREP_MAX_LINE_LENGTH`。
///
/// v0.10.8 (T4): 统一走 `crate::truncate::truncate_line` —— 标记文案
/// （"... [truncated]"）与 Pi 逐字一致。nini 之前用的是「…」，模型看到的
/// 提示与 Pi 不同。
fn truncate_line(line: &str) -> (String, bool) {
    crate::truncate::truncate_line(line, GREP_MAX_LINE_LENGTH)
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
            // v0.10.8 (T1): 与 Pi 0.85.1 的契约逐字对齐
            description: "Search file contents for a pattern. Returns matching lines with \
                          file paths and line numbers. Respects .gitignore. Output is \
                          truncated to 100 matches or 50KB (whichever is hit first). Long \
                          lines are truncated to 500 chars."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Search pattern (regex or literal string)"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory or file to search (default: current directory)"
                    },
                    "glob": {
                        "type": "string",
                        "description": "Filter files by glob pattern, e.g. '*.ts' or '**/*.spec.ts'"
                    },
                    "ignoreCase": {
                        "type": "boolean",
                        "description": "Case-insensitive search (default: false)"
                    },
                    "literal": {
                        "type": "boolean",
                        "description": "Treat pattern as literal string instead of regex (default: false)"
                    },
                    "context": {
                        "type": "number",
                        "description": "Number of lines to show before and after each match (default: 0)"
                    },
                    "limit": {
                        "type": "number",
                        "description": "Maximum number of matches to return (default: 100)"
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

        // `literal`: 当作普通字符串，不当正则。Pi 的 grep 有这个开关，
        // nini 之前没有——模型传了就会报错。
        let pattern = if parsed.literal.unwrap_or(false) {
            regex::escape(&parsed.pattern)
        } else {
            parsed.pattern.clone()
        };
        let mut builder = regex::RegexBuilder::new(&pattern);
        if parsed.ignore_case.unwrap_or(false) {
            builder.case_insensitive(true);
        }
        let regex = builder
            .build()
            .map_err(|e| ToolError::InvalidArgs(format!("invalid regex: {e}")))?;

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

        // Build walker with optional glob filter
        let mut walker = WalkBuilder::new(&path);
        walker.follow_links(false);
        if let Some(ref glob) = parsed.glob {
            let mut ob = OverrideBuilder::new(&path);
            // `!*.ext` excludes, `*.ext` includes — invert: user wants only matching
            if let Err(e) = ob.add(&format!("!{glob}")) {
                return Err(ToolError::InvalidArgs(format!("invalid glob: {e}")));
            }
            walker.overrides(ob.build().expect("valid override"));
        }

        let limit = parsed.limit.unwrap_or(DEFAULT_LIMIT);
        let context = parsed.context.unwrap_or(0) as usize;

        let mut results: Vec<String> = Vec::new();
        let mut files_scanned = 0usize;
        let mut match_count = 0usize;
        let mut truncated_lines = false;

        for entry in walker.build() {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            files_scanned += 1;
            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let lines: Vec<&str> = content.lines().collect();
            // 相对路径（Pi 的 formatPath）：模型看到的是相对 cwd 的路径
            let rel = relative_display(entry.path(), &ctx.cwd);
            for (idx, line) in lines.iter().enumerate() {
                if !regex.is_match(line) {
                    continue;
                }
                if match_count >= limit {
                    break;
                }
                match_count += 1;
                let line_number = idx + 1;
                if context == 0 {
                    let (text, cut) = truncate_line(line);
                    truncated_lines |= cut;
                    results.push(format!("{rel}:{line_number}: {text}"));
                } else {
                    // 上下文块：匹配行是 `path:line: text`，上下文行是 `path-line- text`
                    let start = line_number.saturating_sub(context).max(1);
                    let end = (line_number + context).min(lines.len());
                    for current in start..=end {
                        let (text, cut) = truncate_line(lines[current - 1]);
                        truncated_lines |= cut;
                        if current == line_number {
                            results.push(format!("{rel}:{current}: {text}"));
                        } else {
                            results.push(format!("{rel}-{current}- {text}"));
                        }
                    }
                }
            }
            if match_count >= limit {
                break;
            }
        }

        let mut output = results.join("\n");
        if output.is_empty() {
            output = "No matches found".to_string();
        } else {
            // v0.10.8 (T4): 统一截断（字节上限也算在内）
            let t =
                crate::truncate::truncate_head(&output, ctx.max_output_lines, ctx.max_output_bytes);
            output = t.content;
        }
        if truncated_lines {
            output.push_str(&format!(
                "\n[Long lines truncated to {GREP_MAX_LINE_LENGTH} chars]"
            ));
        }

        Ok(ToolOutput {
            content: output,
            is_error: false,
            details: Some(json!({
                "files_scanned": files_scanned,
                "matches": match_count,
            })),
            duration_ms: 0,
            terminate: false,
        })
    }

    fn system_prompt_contribution(&self) -> Option<ToolSystemPrompt> {
        Some(ToolSystemPrompt {
            // v0.10.8 (T1): Pi 0.85.1 的 snippet 与 guidelines 逐字对齐
            snippet: "Search file contents for patterns (respects .gitignore)".to_string(),
            guidelines: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn grep_spec_is_concise_and_nonoverlapping() {
        // v0.8 regression guard: snippet + description must not duplicate
        // each other, and snippet must be short (Pi-style: ~10 words).
        let tool = GrepTool::new();
        let contrib = tool.system_prompt_contribution().unwrap();
        let word_count = contrib.snippet.split_whitespace().count();
        assert!(
            word_count <= 10,
            "grep snippet too long: {} words — {}",
            word_count,
            contrib.snippet,
        );
        let spec = tool.spec();
        assert!(
            !contrib.snippet.is_empty(),
            "grep snippet must not be empty",
        );
        // Snippet must be a distinct perspective from description —
        // simplest sanity check: neither should be a prefix of the other.
        let s_lower = contrib.snippet.to_lowercase();
        let d_lower = spec.description.to_lowercase();
        assert!(
            !d_lower.starts_with(&s_lower),
            "grep description should not start with the snippet's text",
        );
    }

    #[tokio::test]
    async fn grep_finds_matching_files() {
        let dir = tempdir().unwrap();
        let mut a = std::fs::File::create(dir.path().join("a.txt")).unwrap();
        writeln!(a, "alpha line 1\nbeta line 2").unwrap();
        let mut b = std::fs::File::create(dir.path().join("b.txt")).unwrap();
        writeln!(b, "alpha line\nzzz").unwrap();

        let tool = GrepTool::new();
        let out = tool
            .execute(
                json!({"pattern": "alpha", "path": dir.path().to_str().unwrap()}),
                ToolContext::default(),
            )
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
            .execute(
                json!({"pattern": "nonexistent", "path": dir.path().to_str().unwrap()}),
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(!out.is_error);
        // v0.10.8 (T1): 措辞对齐 Pi 0.85.1 —— Pi 的 grep 在没有匹配时
        // 返回 "No matches found"（dist/tools-t2/grep.js）。这个测试此前
        // 断言的是 nini 自己的旧措辞；旧措辞不是「正确」，只是「当时那样」。
        assert_eq!(out.content, "No matches found");
    }
}
