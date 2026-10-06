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
                        "description": format!(
                            "Maximum number of lines to read. Optional; defaults to \
                             {DEFAULT_READ_MAX_LINES}."
                        )
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
        let offset = parsed.offset.unwrap_or(0) as usize;
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
            snippet: "Read a file's contents into your context.".into(),
            guidelines: vec![
                "Always read a file before editing it — never edit blind.".into(),
                // v0.8.9 (P2-7): the defaults are no longer "everything",
                // so the model has to be told what they are and how to
                // get more. A model that assumes `read` returns the whole
                // file will happily act on a truncated view.
                format!(
                    "A bare `read` returns at most {DEFAULT_READ_MAX_LINES} lines. \
                     Pass `offset` + `limit` to page through the rest; `offset` is \
                     0-indexed."
                ),
                format!(
                    "Files over {} KB are refused outright — use `grep` to find the \
                     relevant lines first.",
                    READ_HARD_MAX_BYTES / 1024
                ),
            ],
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
                json!({"path": path.to_string_lossy(), "offset": 2}),
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
                json!({"path": path.to_string_lossy(), "offset": 1, "limit": u64::MAX}),
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
        assert_eq!(
            out.content.lines().count(),
            DEFAULT_READ_MAX_LINES,
            "exactly the default window should come back"
        );
    }

    #[tokio::test]
    async fn explicit_limit_is_respected() {
        let big: String = (0..5000).map(|i| format!("line {i}\n")).collect();
        let path = write_temp("explicit", &big);
        let tool = ReadTool::new();
        let out = tool
            .execute(
                json!({"path": path.to_string_lossy(), "offset": 10, "limit": 5}),
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(out.content, "line 10\nline 11\nline 12\nline 13\nline 14");
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
        let contrib = ReadTool::new().system_prompt_contribution().unwrap();
        let joined = contrib.guidelines.join(" ");
        assert!(
            joined.contains(&DEFAULT_READ_MAX_LINES.to_string()),
            "guidelines must state the default line cap: {joined}"
        );
        assert!(
            joined.contains("offset"),
            "guidelines must explain how to get more: {joined}"
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
