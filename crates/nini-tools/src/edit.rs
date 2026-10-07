//! `edit` tool: surgically replace a string in a file.
//!
//! Spec mirrors `packages/coding-agent/src/core/tools/edit.ts`:
//! - `oldText` must match exactly once (errors if not found or matches many)
//! - Replaces the first occurrence with `newText`
//! - Atomic write via temp file + rename

use async_trait::async_trait;
use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolSpec, ToolSystemPrompt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::diff;
use tokio::fs;

/// One replacement in Pi's `edit` contract.
///
/// The field names are Pi's exactly - `oldText` / `newText` - because the
/// model is told these names in the system prompt and emits them verbatim.
/// Snake_case is kept as an *alias* so existing nini callers keep working;
/// advertising a second spelling in the schema would not, because a model
/// reading two spellings picks neither reliably.
#[derive(Debug, Clone, Deserialize)]
struct EditItem {
    #[serde(rename = "oldText", alias = "old_text")]
    old_text: String,
    #[serde(rename = "newText", alias = "new_text")]
    new_text: String,
}

#[derive(Debug, Clone, Deserialize)]
struct EditArgs {
    path: String,
    /// Pi's shape: several disjoint replacements in one call.
    #[serde(default)]
    edits: Option<Vec<EditItem>>,
    /// nini's pre-v0.10 shape, still accepted.
    #[serde(default)]
    old_text: Option<String>,
    #[serde(default)]
    new_text: Option<String>,
}

impl EditArgs {
    /// Normalise both accepted shapes into one list of replacements.
    ///
    /// Takes `&self` rather than consuming: the caller still needs `path`
    /// afterwards, and cloning two Strings is cheaper than the borrow
    /// gymnastics of partial moves.
    fn into_edits(&self) -> Result<Vec<EditItem>, String> {
        if let Some(edits) = &self.edits {
            if edits.is_empty() {
                return Err("`edits` must not be empty".to_string());
            }
            return Ok(edits.to_vec());
        }
        match (&self.old_text, &self.new_text) {
            (Some(o), Some(n)) => Ok(vec![EditItem {
                old_text: o.clone(),
                new_text: n.clone(),
            }]),
            (None, None) => {
                Err("edit requires `edits` (Pi contract) or `old_text` + `new_text`".to_string())
            }
            _ => Err("`old_text` and `new_text` must be given together".to_string()),
        }
    }
}

#[derive(Debug, Default)]
pub struct EditTool;

impl EditTool {
    pub fn new() -> Self {
        Self
    }

    /// Apply every replacement synchronously (no I/O).
    ///
    /// v0.10: this takes a *list*. `edit` previously handled exactly one
    /// replacement, so a model following Pi's system prompt ("use one edit
    /// call with multiple entries in edits[]") was rejected outright.
    ///
    /// Replacements are applied in order, each matched against the buffer as
    /// it stands. That reproduces Pi's observed behaviour rather than its
    /// prose: probed against the real binary, a self-overlapping replacement
    /// (`ALPHA` -> `<1-ALPHA>`) nests instead of stopping after one pass.
    /// Pi's prompt forbids such edits ("Do not emit overlapping or nested
    /// edits"), so the strategies are indistinguishable for every input Pi
    /// considers valid - but matching the observed behaviour means a
    /// Pi-produced tool call replays identically here.
    fn apply_all(content: &str, edits: &[EditItem]) -> Result<(String, usize), String> {
        let mut out = content.to_string();
        for (i, e) in edits.iter().enumerate() {
            if e.old_text.is_empty() {
                return Err(format!("edits[{i}].oldText must not be empty"));
            }
            let occurrences = out.matches(&e.old_text).count();
            match occurrences {
                0 => {
                    return Err(format!(
                        "Could not find edits[{i}]. The oldText must match exactly \
                         including all whitespace and newlines."
                    ))
                }
                1 => out = out.replacen(&e.old_text, &e.new_text, 1),
                n => {
                    return Err(format!(
                        "edits[{i}].oldText matches {n} locations; must match exactly 1"
                    ))
                }
            }
        }
        Ok((out, edits.len()))
    }

    /// Pi's tool_result wording, reproduced from a real capture:
    /// "Successfully replaced 3 block(s) in <path>."
    fn success_message(path: &Path, n: usize) -> String {
        format!("Successfully replaced {n} block(s) in {}.", path.display())
    }
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".to_string(),
            // Pi 0.84.2's description, captured verbatim. It carries the
            // non-overlap guidance the snippet deliberately omits, so the
            // two must stay different (a v0.8 guard enforces this).
            description: "Edit a single file using exact text replacement.\
                          Every edits[].oldText must match a unique,\"
                          non-overlapping region of the original file. If two\"
                          changes affect the same block or nearby lines, merge\"
                          them into one edit instead of emitting overlapping\"
                          edits. Do not include large unchanged regions just\"
                          to connect distant changes."
                .to_string(),
            // Pi's schema, captured from 0.84.2 at runtime. `old_text` /
            // `new_text` stay accepted on the way in but are NOT advertised:
            // the model must learn one spelling, and it is the one Pi uses.
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to edit (relative or absolute)"
                    },
                    "edits": {
                        "type": "array",
                        "description": "One or more replacements applied in a single call.",
                        "items": {
                            "type": "object",
                            "required": ["oldText", "newText"],
                            "properties": {
                                "oldText": {
                                    "type": "string",
                                    "description": "Exact text for one targeted replacement. \
                                                   It must be unique in the file."
                                },
                                "newText": {
                                    "type": "string",
                                    "description": "Replacement text."
                                }
                            }
                        }
                    }
                },
                "required": ["path", "edits"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let parsed: EditArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArgs(e.to_string()))?;
        let edits = parsed.into_edits().map_err(ToolError::InvalidArgs)?;

        let path = if Path::new(&parsed.path).is_absolute() {
            PathBuf::from(&parsed.path)
        } else {
            ctx.cwd.join(&parsed.path)
        };

        // v0.10 (G2): the read-modify-write below is now atomic per file.
        //
        // nini runs a batch of tool calls concurrently, so two edits to the
        // same path in one assistant turn — an ordinary pattern, e.g. two
        // unrelated replacements in one file — both read the same original
        // and the second silently overwrote the first. Measured here with 8
        // concurrent edits: 1–4 of 8 reported success and 1–2 of 8 survived.
        //
        // Pi wraps `edit` and `write` in `withFileMutationQueue`
        // (`pi-agent-core/src/harness/tools/file-mutation-queue.ts`), keyed
        // on the canonical path, so the same file serialises while different
        // files stay parallel. The lock lives here rather than in the agent
        // so the guarantee holds for every caller.
        crate::mutation_queue::with_file_mutation_queue(&path, || async {
            let content = fs::read_to_string(&path)
                .await
                .map_err(|e| match e.kind() {
                    std::io::ErrorKind::NotFound => {
                        ToolError::Io(format!("file not found: {}", path.display()))
                    }
                    _ => ToolError::Io(e.to_string()),
                })?;

            let (new_content, replacements) =
                Self::apply_all(&content, &edits).map_err(ToolError::InvalidArgs)?;

            // Atomic write
            let parent = path.parent().unwrap_or(Path::new("."));
            let file_name = path.file_name().unwrap();
            // v0.10 (G2): see the note in `write.rs` — the pid-only temp name
            // collided between concurrent edits to the same file.
            let tmp = parent.join(crate::mutation_queue::unique_temp_name(
                &file_name.to_string_lossy(),
            ));
            fs::write(&tmp, new_content.as_bytes()).await?;
            if let Err(e) = fs::rename(&tmp, &path).await {
                let _ = fs::remove_file(&tmp).await;
                return Err(ToolError::Io(e.to_string()));
            }

            // Build a unified diff for the TUI to render with +/- coloring.
            // Context of 3 matches `diff -u` default behavior.
            let diff_text = diff::render_unified(&content, &new_content, 3);
            let (adds, dels) = diff::diff_summary(&content, &new_content);

            Ok(ToolOutput {
                content: format!(
                    "{}\n\n{}",
                    Self::success_message(&path, replacements),
                    diff_text
                ),
                is_error: false,
                details: Some(json!({
                    "path": path.display().to_string(),
                    "replacements": replacements,
                    "additions": adds,
                    "deletions": dels,
                    "diff": diff_text,
                })),
                duration_ms: 0,
                terminate: false,
            })
        })
        .await
    }

    fn system_prompt_contribution(&self) -> Option<ToolSystemPrompt> {
        Some(ToolSystemPrompt {
            snippet: "Make precise file edits with exact text replacement, \
                      including multiple disjoint edits in one call."
                .into(),
            guidelines: vec![
                "Use edit for precise changes (edits[].oldText must match exactly)".into(),
                "When changing multiple separate locations in one file, use one edit \
                 call with multiple entries in edits[] instead of multiple edit calls"
                    .into(),
                "Each edits[].oldText must be unique in the file. Do not emit \
                 overlapping or nested edits."
                    .into(),
                "Keep edits[].oldText as small as possible while still being unique.".into(),
                "For whole-file rewrites use `write`, not edit.".into(),
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_file(content: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, content).unwrap();
        (dir, path)
    }

    #[tokio::test]
    async fn edit_replaces_single_occurrence() {
        let (_dir, path) = make_file("hello world\n");
        let tool = EditTool::new();
        let out = tool
            .execute(
                json!({"path": path.to_str().unwrap(), "old_text": "world", "new_text": "rust"}),
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(!out.is_error, "got: {:?}", out);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello rust\n");
    }

    #[tokio::test]
    async fn edit_rejects_zero_matches() {
        let (_dir, path) = make_file("hello world\n");
        let tool = EditTool::new();
        let err = tool
            .execute(
                json!({"path": path.to_str().unwrap(), "old_text": "missing", "new_text": "x"}),
                ToolContext::default(),
            )
            .await;
        assert!(matches!(err, Err(ToolError::InvalidArgs(_))));
    }

    #[tokio::test]
    async fn edit_rejects_multiple_matches() {
        let (_dir, path) = make_file("foo foo foo\n");
        let tool = EditTool::new();
        let err = tool
            .execute(
                json!({"path": path.to_str().unwrap(), "old_text": "foo", "new_text": "bar"}),
                ToolContext::default(),
            )
            .await;
        assert!(matches!(err, Err(ToolError::InvalidArgs(_))));
    }

    #[tokio::test]
    async fn edit_rejects_empty_old_text() {
        let (_dir, path) = make_file("hello\n");
        let tool = EditTool::new();
        let err = tool
            .execute(
                json!({"path": path.to_str().unwrap(), "old_text": "", "new_text": "x"}),
                ToolContext::default(),
            )
            .await;
        assert!(matches!(err, Err(ToolError::InvalidArgs(_))));
    }

    #[test]
    fn edit_spec_is_concise_and_nonoverlapping() {
        // v0.8 regression guard: snippet + description must not duplicate
        // each other, and snippet must be short.
        //
        // v0.10: the bound moved from 10 to 16 words, and that is Pi's
        // doing rather than nini's. Pi 0.84.2's captured `edit` snippet is
        //
        //   "Make precise file edits with exact text replacement, including
        //    multiple disjoint edits in one call"
        //
        // — 15 words. The clause "including multiple disjoint edits in one
        // call" is the only thing that teaches a model the tool accepts a
        // list, and Pi's system prompt leans on it hard (five guidelines
        // about `edits[]`). Trimming nini's snippet back under 10 words to
        // satisfy a self-imposed limit would have re-broken the very
        // capability the `edits[]` work just added, while the schema
        // quietly said otherwise.
        //
        // The limit is still a limit: 16 is a ceiling, not a target.
        let tool = EditTool::new();
        let contrib = tool.system_prompt_contribution().unwrap();
        let word_count = contrib.snippet.split_whitespace().count();
        assert!(
            word_count <= 16,
            "edit snippet too long: {} words — {}",
            word_count,
            contrib.snippet,
        );
        assert!(
            contrib.snippet.contains("multiple disjoint edits"),
            "the snippet must keep telling the model the tool takes a list, \
             otherwise the multi-edit capability is undocumented: {}",
            contrib.snippet
        );
        let spec = tool.spec();
        assert!(
            !contrib.snippet.is_empty(),
            "edit snippet must not be empty",
        );
        // Snippet must be a distinct perspective from description —
        // simplest sanity check: neither should be a prefix of the other.
        let s_lower = contrib.snippet.to_lowercase();
        let d_lower = spec.description.to_lowercase();
        assert!(
            !d_lower.starts_with(&s_lower),
            "edit description should not start with the snippet's text",
        );
    }

    /// v0.10: `apply` took a single replacement and became `apply_all`,
    /// which takes Pi's list. The per-replacement rules are unchanged; what
    /// is new is that a whole batch fails atomically if any member does.
    fn one(old: &str, new: &str) -> Vec<EditItem> {
        vec![EditItem {
            old_text: old.to_string(),
            new_text: new.to_string(),
        }]
    }

    #[test]
    fn apply_logic() {
        let (new, n) = EditTool::apply_all("abc xyz", &one("abc", "x")).unwrap();
        assert_eq!(new, "x xyz");
        assert_eq!(n, 1);
        assert!(EditTool::apply_all("abc abc", &one("abc", "x")).is_err()); // 2 matches
        assert!(EditTool::apply_all("xyz", &one("abc", "x")).is_err()); // 0 matches
        assert!(EditTool::apply_all("any", &one("", "x")).is_err()); // empty old
    }

    /// The capability that did not exist before v0.10: several disjoint
    /// replacements in one call, which is what Pi's system prompt tells the
    /// model to emit.
    #[test]
    fn apply_all_handles_several_disjoint_replacements() {
        let edits = vec![
            EditItem {
                old_text: "ALPHA".into(),
                new_text: "X1X".into(),
            },
            EditItem {
                old_text: "BETA".into(),
                new_text: "X2X".into(),
            },
            EditItem {
                old_text: "GAMMA".into(),
                new_text: "X3X".into(),
            },
        ];
        let (new, n) = EditTool::apply_all("ALPHA\nBETA\nGAMMA\n", &edits).unwrap();
        assert_eq!(new, "X1X\nX2X\nX3X\n");
        assert_eq!(n, 3);
    }

    /// A batch must be all-or-nothing: a failing member leaves the content
    /// untouched, because the caller writes once at the end.
    #[test]
    fn a_failing_member_aborts_the_whole_batch() {
        let edits = vec![
            EditItem {
                old_text: "ALPHA".into(),
                new_text: "X1X".into(),
            },
            EditItem {
                old_text: "MISSING".into(),
                new_text: "X2X".into(),
            },
        ];
        let err = EditTool::apply_all("ALPHA\n", &edits).unwrap_err();
        assert!(err.contains("edits[1]"), "{err}");
    }
}

#[cfg(test)]
mod diff_integration_tests {

    use crate::diff::{diff_summary, render_unified};

    #[test]
    fn edit_returns_diff_in_details() {
        let (adds, dels) = diff_summary("line1\nline2\nline3\n", "line1\nmodified\nline3\n");
        assert_eq!(adds, 1);
        assert_eq!(dels, 1);
    }

    #[test]
    fn edit_diff_renders_unified_format() {
        let d = render_unified("a\nb\nc\n", "a\nB\nc\n", 1);
        // Should contain a `-b`, `+B`, and context lines.
        assert!(d.lines().any(|l| l.starts_with("-b")));
        assert!(d.lines().any(|l| l.starts_with("+B")));
        assert!(d.lines().any(|l| l == " a"));
        assert!(d.lines().any(|l| l == " c"));
    }
}
