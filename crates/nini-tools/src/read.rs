//! `read` tool: read a file (with image/http passthrough deferred to Phase 3).

use async_trait::async_trait;
use nini_core::tool::*;
use serde::Deserialize;
use serde_json::{json, Value};
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

/// Lines returned when the caller does not pass `limit`.
///
/// v0.8.9 (P2-7): was `usize::MAX`, i.e. "read the whole file no matter
/// how big". Two problems, one of them a hard crash:
///
/// * `offset + limit` with `limit = usize::MAX` **overflows**. In a
///   debug build (which is what `cargo test` runs) that panics with
///   `attempt to add with overflow`; in release it wraps to `offset - 1`
///   and the tool silently returns an empty string, so the model
///   concludes the file is empty. Either way, a model that passes
///   `offset` without `limit` — a completely reasonable thing to do —
///   gets a crash or a lie.
/// * a 2 GB file was loaded into memory in full, then
///   `content.lines().collect()` allocated a second `Vec<&str>` of
///   ~30 million fat pointers on top.
pub const DEFAULT_READ_MAX_LINES: usize = 2_000;

/// Hard ceiling on the file size the tool will load at all.
///
/// Checked against the file's metadata *before* reading, so a huge file
/// is refused instead of being paged in and then thrown away. Well above
/// the point where chunking with `offset` + `limit` is the right move.
pub const READ_HARD_MAX_BYTES: u64 = 4 * 1024 * 1024;

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
            // v0.10.8 (T1): 与 Pi 0.85.1 的契约逐字对齐
            // （references/pi-0.85.1/tool-contracts.json，由 scripts/pi-tool-dump.mjs
            //  执行 Pi 自己的代码产出）。
            description: "Read the contents of a file. Supports text files and images \
                          (jpg, png, gif, webp, bmp). Images are sent as attachments. For \
                          text files, output is truncated to 2000 lines or 50KB (whichever \
                          is hit first). Use offset/limit for large files. When you need \
                          the full file, continue with offset until complete."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to read (relative or absolute)"
                    },
                    "offset": {
                        "type": "number",
                        "description": "Line number to start reading from (1-indexed)"
                    },
                    "limit": {
                        "type": "number",
                        "description": "Maximum number of lines to read"
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

        // v0.8.9 (P2-7): check the size *before* reading. The old code
        // called `read_to_string` on whatever it was pointed at, so the
        // `limit` was a limit on what got *returned*, never on what got
        // *loaded*.
        if let Ok(meta) = tokio::fs::metadata(&path).await {
            if meta.len() > READ_HARD_MAX_BYTES {
                return Err(ToolError::InvalidArgs(format!(
                    "{} is {} KB, over the {} KB read ceiling. Use `grep` to \
                     locate the part you need, or read it in chunks — but note \
                     the ceiling is on the whole file, so a file this size \
                     cannot be read directly.",
                    path.display(),
                    meta.len() / 1024,
                    READ_HARD_MAX_BYTES / 1024
                )));
            }
        }

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

        let total_lines = content.lines().count();
        // v0.10.8 (T1): `offset` 变成 **1-indexed**，与 Pi 0.85.1 一致
        // （契约原文："Line number to start reading from (1-indexed)"；
        //  Pi 的实现是 `Math.max(0, offset - 1)`）。
        // 之前 nini 是 0-indexed，而契约（对齐后）说的是 1-indexed ——
        // **契约说谎比不一致更糟**：模型按 1-indexed 读，会错位一行。
        // 换算：offset=1 → 第一行；offset=None → 第一行。
        let offset = parsed
            .offset
            .map(|n| n.saturating_sub(1) as usize)
            .unwrap_or(0);
        let limit = parsed
            .limit
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_READ_MAX_LINES);
        // `saturating_add`, not `+`: with a caller-supplied `offset` and
        // no `limit` the old expression overflowed. Debug builds panic
        // (`attempt to add with overflow`); release wraps and the tool
        // returns an empty string, so the model believes the file has no
        // content past the offset.
        let end = offset.saturating_add(limit).min(total_lines);
        let start = offset.min(total_lines);

        // Walk lazily rather than materialising every line as a `&str`.
        // For a 4 MB file the old `lines().collect()` allocated a
        // ~60 000-element pointer vector on top of the string we already
        // hold, for a slice we then throw most of away.
        let selected: String = if start >= end {
            String::new()
        } else {
            content
                .lines()
                .skip(start)
                .take(end - start)
                .collect::<Vec<_>>()
                .join("\n")
        };

        // v0.10.8 (T4): 统一截断 + Pi 的「这是窗口」标记。
        //
        // Pi 的三种标记（dist/tools-t1/read.js）：
        //   "[Showing lines A-B of N. Use offset=N+1 to continue.]"
        //   "[Showing lines A-B of N (50KB limit). Use offset=N+1 to continue.]"
        //   "[M more lines in file. Use offset=N+1 to continue.]"
        // nini 此前**一个标记都没有**——模型看不出自己拿到的是窗口而不是全文。
        let mut selected = selected;
        let mut truncated_by_bytes = false;
        if total_lines > 0 {
            let t = crate::truncate::truncate_head(&selected, limit.max(1), ctx.max_output_bytes);
            truncated_by_bytes = t.truncated_by == Some("bytes");
            selected = t.content;
        }
        let next_offset = end + 1;
        if end < total_lines {
            if truncated_by_bytes {
                selected.push_str(&format!(
                    "\n\n[Showing lines {}-{} of {} ({} limit). Use offset={next_offset} to continue.]",
                    start + 1,
                    end,
                    total_lines,
                    crate::truncate::format_size(ctx.max_output_bytes),
                ));
            } else {
                selected.push_str(&format!(
                    "\n\n[Showing lines {}-{} of {}. Use offset={next_offset} to continue.]",
                    start + 1,
                    end,
                    total_lines,
                ));
            }
        }

        let details = json!({
            "path": path.display().to_string(),
            "total_lines": total_lines,
            "offset": offset,
            "limit": limit,
            // v0.8.9 (P2-7): the model needs to know the window it got
            // is a window, not the file. Without this it has no way to
            // tell "the rest of this file is empty" from "I only asked
            // for part of it".
            "returned_lines": end.saturating_sub(start),
            "truncated": end < total_lines,
            "default_limit": parsed.limit.is_none(),
        });

        Ok(ToolOutput {
            content: selected,
            is_error: false,
            details: Some(details),
            duration_ms: 0,
            terminate: false,
        })
    }

    fn system_prompt_contribution(&self) -> Option<nini_core::tool::ToolSystemPrompt> {
        Some(nini_core::tool::ToolSystemPrompt {
            // v0.10.8 (T1): Pi 的 snippet 与 guidelines 逐字对齐
            snippet: "Read file contents".into(),
            guidelines: vec!["Use read to examine files instead of cat or sed.".into()],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use tempfile::NamedTempFile;

    fn write_temp(name: &str, contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("nini-read-test-{name}"));
        std::fs::write(&path, contents).unwrap();
        path
    }

    // ---- v0.8.9 (P2-7) regression guards -----------------------------
    //
    // The headline bug: `limit` defaulted to `usize::MAX`, so
    // `offset + limit` overflowed. Debug builds (which is what
    // `cargo test` runs, and what a developer runs) panicked with
    // `attempt to add with overflow`; release wrapped to `offset - 1`
    // and the tool silently returned an empty string, so the model
    // concluded the file had no content past the offset. A model
    // passing `offset` without `limit` is a completely ordinary thing
    // to do, and it killed the agent.

    #[tokio::test]
    async fn offset_without_limit_does_not_overflow() {
        let path = write_temp("offset-no-limit", "l0\nl1\nl2\nl3\nl4\n");
        let tool = ReadTool::new();
        // offset within the file: must return the tail, not panic and
        // not silently return "".
        let out = tool
            .execute(
                // v0.10.8 (T1): offset 是 1-indexed（与 Pi 0.85.1 一致），
                // 所以「第 3 行开始」写 3（旧语义下同样的位置要写 2）。
                json!({"path": path.to_string_lossy(), "offset": 3}),
                ToolContext::default(),
            )
            .await
            .expect("offset without limit must not fail");
        assert_eq!(
            out.content, "l2\nl3\nl4",
            "expected the lines from offset 2 onward"
        );

        // offset past EOF: empty is correct here, and must not panic.
        let out2 = tool
            .execute(
                json!({"path": path.to_string_lossy(), "offset": 99_999}),
                ToolContext::default(),
            )
            .await
            .expect("offset past EOF must not fail");
        assert!(out2.content.is_empty());
    }

    #[tokio::test]
    async fn huge_limit_does_not_overflow() {
        let path = write_temp("huge-limit", "a\nb\nc\n");
        let tool = ReadTool::new();
        let out = tool
            .execute(
                json!({"path": path.to_string_lossy(), "offset": 2, "limit": u64::MAX}),
                ToolContext::default(),
            )
            .await
            .expect("u64::MAX limit must not fail");
        assert_eq!(out.content, "b\nc");
    }

    #[tokio::test]
    async fn bare_read_is_capped_and_says_so() {
        // The new default is not "the whole file" any more, so the model
        // has to be able to tell that. A silently truncated read is a
        // lie: the model would act on a partial view believing it is
        // complete.
        let big: String = (0..5000).map(|i| format!("line {i}\n")).collect();
        let path = write_temp("big", &big);
        let tool = ReadTool::new();
        let out = tool
            .execute(
                json!({"path": path.to_string_lossy()}),
                ToolContext::default(),
            )
            .await
            .unwrap();

        let details = out.details.unwrap();
        assert_eq!(details["total_lines"], 5000);
        assert_eq!(details["limit"], DEFAULT_READ_MAX_LINES);
        assert_eq!(details["returned_lines"], DEFAULT_READ_MAX_LINES);
        assert_eq!(details["truncated"], true);
        assert_eq!(
            details["default_limit"], true,
            "the model should be able to tell the cap was not its idea"
        );
        // v0.10.8 (T4): 末尾多了 Pi 的「这是窗口」标记，计数只看窗口本身
        let window = out.content.split("\n\n[").next().unwrap_or(&out.content);
        assert_eq!(
            window.lines().count(),
            DEFAULT_READ_MAX_LINES,
            "exactly the default window should come back"
        );
        assert!(
            out.content.contains("Use offset=2001 to continue."),
            "必须告诉模型怎么继续：{}",
            out.content
        );
    }

    #[tokio::test]
    async fn explicit_limit_is_respected() {
        let big: String = (0..5000).map(|i| format!("line {i}\n")).collect();
        let path = write_temp("explicit", &big);
        let tool = ReadTool::new();
        let out = tool
            .execute(
                json!({"path": path.to_string_lossy(), "offset": 11, "limit": 5}),
                ToolContext::default(),
            )
            .await
            .unwrap();
        // v0.10.8 (T4): 末尾带 Pi 的「这是窗口」标记
        assert!(
            out.content
                .starts_with("line 10\nline 11\nline 12\nline 13\nline 14"),
            "窗口内容不对：{}",
            out.content
        );
        assert!(
            out.content
                .contains("[Showing lines 11-15 of 5000. Use offset=16 to continue.]"),
            "必须告诉模型这是窗口、以及怎么继续：{}",
            out.content
        );
        let details = out.details.unwrap();
        assert_eq!(details["default_limit"], false);
        assert_eq!(details["truncated"], true);
    }

    #[tokio::test]
    async fn short_file_is_not_marked_truncated() {
        let path = write_temp("short", "one\ntwo\n");
        let tool = ReadTool::new();
        let out = tool
            .execute(
                json!({"path": path.to_string_lossy()}),
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(out.content, "one\ntwo");
        assert_eq!(
            out.details.unwrap()["truncated"],
            false,
            "a file that fit must not be reported as truncated"
        );
    }

    #[tokio::test]
    async fn oversized_file_is_refused_before_loading() {
        // The ceiling is checked against metadata, so a huge file is
        // never paged into memory just to be truncated afterwards. The
        // error has to tell the model what to do instead.
        let path = std::env::temp_dir().join("nini-read-test-huge.bin");
        // Sparse file: seek past the limit and write one byte, so this
        // costs no real disk space.
        let f = std::fs::File::create(&path).unwrap();
        {
            use std::io::{Seek, SeekFrom};
            let mut f = f;
            f.seek(SeekFrom::Start(READ_HARD_MAX_BYTES + 1)).unwrap();
            f.write_all(b"x").unwrap();
        }
        let tool = ReadTool::new();
        let err = tool
            .execute(
                json!({"path": path.to_string_lossy()}),
                ToolContext::default(),
            )
            .await
            .expect_err("a file over the ceiling must be refused");
        let msg = err.to_string();
        assert!(
            msg.contains("grep"),
            "the error should suggest a way forward, got: {msg}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn system_prompt_states_the_new_defaults() {
        // A model that assumes `read` returns everything will act on a
        // truncated view. The limits have to be in the prompt.
        //
        // v0.10.8 (T1): Pi 0.85.1 把这条信息放在**description** 里，
        // 而它的 guidelines 只有一句 "Use read to examine files instead of
        // cat or sed."。所以这条性质改在 Pi 的载体上断言——**性质不变，
        // 载体对齐**。测试断言的是「prompt 里必须写明上限与翻页方式」。
        let tool = ReadTool::new();
        let desc = tool.spec().description;
        assert!(
            desc.contains("2000 lines") && desc.contains("50KB"),
            "description must state the truncation caps: {desc}"
        );
        assert!(
            desc.contains("offset") && desc.contains("limit"),
            "description must explain how to page: {desc}"
        );
        let contrib = tool.system_prompt_contribution().unwrap();
        assert_eq!(
            contrib.guidelines.join(" "),
            "Use read to examine files instead of cat or sed.",
            "guidelines must match Pi 0.85.1 verbatim"
        );
    }

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
            word_count,
            contrib.snippet,
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
                // v0.10.8 (T1): 1-indexed —— 第 4 行开始（旧的 0-indexed 语义下是 3）
                json!({"path": path, "offset": 4, "limit": 2}),
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(
            out.content.starts_with("line 3\nline 4"),
            "窗口内容不对：{}",
            out.content
        );
        assert!(
            out.content.contains("Use offset=6 to continue."),
            "必须告诉模型怎么继续：{}",
            out.content
        );
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
