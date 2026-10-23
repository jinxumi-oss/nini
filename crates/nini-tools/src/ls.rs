//! `ls` tool: list a directory's contents.
//!
//! v0.10.8 (T2)：nini 此前**没有** `ls`——而 Pi 0.85.1 有它，而且它不是
//! 「锦上添花」的工具：
//!
//! * Pi 的 system prompt 按**已启用的工具集**决定引导语
//!   （`references/pi-0.85.1/dist/api/system-prompt.js`：
//!   `hasBash && !hasGrep && !hasFind && !hasLs` → "Use bash for file
//!   operations like ls, rg, find"）——工具集变了，**模型的行为就变了**；
//! * `ls` 的输出是**给模型解析的**：条目按字母序、目录带 `/` 后缀、含
//!   dotfile、截断时给出可执行的提示（"Use limit=1000 for more"）。
//!
//! 语义对齐 Pi 0.85.1 的 `dist/tools-t2/ls.js`：
//!   * 路径相对 cwd 解析；不存在 / 不是目录 / 读不了 → 各自的错误；
//!   * 按**大小写不敏感**的字母序排序；
//!   * 目录条目带 `/` 后缀；
//!   * `limit` 默认 500；到顶时给提示："500 entries limit reached.
//!     Use limit=1000 for more"；
//!   * 空目录 → "(empty directory)"；
//!   * 字节截断 50KB（与其它工具同一套截断，见 truncate 模块）。

use async_trait::async_trait;
use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolSpec, ToolSystemPrompt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Pi 的默认条目数上限。
const DEFAULT_LIMIT: usize = 500;

#[derive(Debug, Deserialize)]
struct LsArgs {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Default)]
pub struct LsTool;

impl LsTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for LsTool {
    fn name(&self) -> &'static str {
        "ls"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ls".to_string(),
            // v0.10.8 (T1/T2): 与 Pi 0.85.1 的契约逐字对齐
            // （references/pi-0.85.1/tool-contracts.json，由 scripts/pi-tool-dump.mjs
            //  执行 Pi 自己的代码产出）。
            description: "List directory contents. Returns entries sorted alphabetically, \
                          with '/' suffix for directories. Includes dotfiles. Output is \
                          truncated to 500 entries or 50KB (whichever is hit first)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory to list (default: current directory)"
                    },
                    "limit": {
                        "type": "number",
                        "description": "Maximum number of entries to return (default: 500)"
                    }
                },
                "required": [],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let parsed: LsArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArgs(e.to_string()))?;

        let dir_path: PathBuf = match parsed.path.as_deref() {
            None | Some("") => ctx.cwd.clone(),
            Some(p) => {
                let raw = Path::new(p);
                if raw.is_absolute() {
                    raw.to_path_buf()
                } else {
                    ctx.cwd.join(raw)
                }
            }
        };

        if !dir_path.exists() {
            return Err(ToolError::Io(format!(
                "Path not found: {}",
                dir_path.display()
            )));
        }
        if !dir_path.is_dir() {
            return Err(ToolError::Io(format!(
                "Not a directory: {}",
                dir_path.display()
            )));
        }

        let mut entries: Vec<String> = std::fs::read_dir(&dir_path)
            .map_err(|e| ToolError::Io(format!("Cannot read directory: {e}")))?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();

        // 大小写不敏感的字母序（Pi 的 localeCompare 近似）
        entries.sort_by(|a, b| {
            a.to_lowercase()
                .cmp(&b.to_lowercase())
                .then_with(|| a.cmp(b))
        });

        let limit = parsed.limit.unwrap_or(DEFAULT_LIMIT);
        let mut entry_limit_reached = false;
        let mut results: Vec<String> = Vec::with_capacity(entries.len());
        for entry in entries {
            if results.len() >= limit {
                entry_limit_reached = true;
                break;
            }
            let full = dir_path.join(&entry);
            let suffix = match std::fs::metadata(&full) {
                Ok(md) if md.is_dir() => "/",
                Ok(_) => "",
                // 读不到 stat 的条目：Pi 直接跳过
                Err(_) => continue,
            };
            results.push(format!("{entry}{suffix}"));
        }

        if results.is_empty() {
            return Ok(ToolOutput {
                content: "(empty directory)".to_string(),
                is_error: false,
                details: None,
                duration_ms: 0,
                terminate: false,
            });
        }

        let mut output = results.join("\n");
        let mut notices: Vec<String> = Vec::new();
        // v0.10.8 (T4): 统一截断（与其它工具同一套：行数 + 字节，先到者胜）
        let t = crate::truncate::truncate_head(&output, ctx.max_output_lines, ctx.max_output_bytes);
        output = t.content;
        let truncated = t.truncated;
        if entry_limit_reached {
            notices.push(format!(
                "{limit} entries limit reached. Use limit={} for more",
                limit * 2
            ));
        }
        if truncated {
            notices.push("50KB limit reached".to_string());
        }
        if !notices.is_empty() {
            output.push_str(&format!("\n\n[{}]", notices.join(". ")));
        }

        Ok(ToolOutput {
            content: output,
            is_error: false,
            details: Some(json!({
                "entries": results.len(),
                "entryLimitReached": entry_limit_reached,
            })),
            duration_ms: 0,
            terminate: false,
        })
    }

    fn system_prompt_contribution(&self) -> Option<ToolSystemPrompt> {
        Some(ToolSystemPrompt {
            // v0.10.8 (T1): Pi 0.85.1 的 snippet 与 guidelines 逐字对齐
            snippet: "List directory contents".to_string(),
            guidelines: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &Path) -> ToolContext {
        ToolContext {
            cwd: dir.to_path_buf(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn lists_entries_sorted_with_directory_suffix() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.txt"), "x").unwrap();
        std::fs::write(dir.path().join("A.txt"), "x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();

        let out = LsTool::new()
            .execute(json!({}), ctx(dir.path()))
            .await
            .expect("ls 应成功");
        let lines: Vec<&str> = out.content.lines().collect();
        assert_eq!(
            lines,
            vec!["A.txt", "b.txt", "sub/"],
            "排序与目录后缀要对齐 Pi"
        );
    }

    #[tokio::test]
    async fn reports_entry_limit_with_an_actionable_notice() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}")), "x").unwrap();
        }
        let out = LsTool::new()
            .execute(json!({"limit": 2}), ctx(dir.path()))
            .await
            .expect("ls 应成功");
        assert!(
            out.content
                .contains("2 entries limit reached. Use limit=4 for more"),
            "到顶时必须给出可执行的提示：{}",
            out.content
        );
    }

    #[tokio::test]
    async fn empty_directory_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let out = LsTool::new()
            .execute(json!({}), ctx(dir.path()))
            .await
            .expect("ls 应成功");
        assert_eq!(out.content, "(empty directory)");
    }

    #[tokio::test]
    async fn missing_path_is_an_error_not_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let err = LsTool::new()
            .execute(json!({"path": "nope"}), ctx(dir.path()))
            .await
            .expect_err("不存在的路径必须是错误");
        assert!(
            err.to_string().contains("Path not found"),
            "错误文案要对齐 Pi：{err}"
        );
    }

    #[tokio::test]
    async fn a_file_is_not_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f.txt");
        std::fs::write(&f, "x").unwrap();
        let err = LsTool::new()
            .execute(json!({"path": f.to_string_lossy()}), ctx(dir.path()))
            .await
            .expect_err("对着文件 ls 必须是错误");
        assert!(err.to_string().contains("Not a directory"), "{err}");
    }
}
