//! Prompt Templates: user-defined slash-command-style prompts loaded
//! from `.md` files with YAML frontmatter.
//!
//! Mirrors Pi's `loadPromptTemplates(env, paths)` in `dist/harness/prompt-templates.js`:
//! - Directory inputs load direct `.md` children non-recursively.
//! - File inputs load explicit `.md` files.
//! - Missing paths and non-markdown files are skipped.
//! - Read and parse failures are returned as diagnostics.
//! - Frontmatter (between leading `---\n` and `\n---`) is parsed as YAML.
//!   `description: ...` is extracted for `/help` display.
//!
//! File format:
//!
//! ```markdown
//! ---
//! description: Explain a code snippet
//! ---
//! Please explain the following code:
//!
//! ```
//! $1
//! ```
//! ```
//!
//! `$1`, `$@`, `$ARGUMENTS`, `${@:N}`, `${@:N:L}` placeholders are
//! substituted at invocation time. Use `/prompt <name> [args]` in the
//! CLI to invoke.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// A user-defined prompt template.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptTemplate {
    /// Template name (derived from filename, without `.md`).
    pub name: String,
    /// Human-readable description (from frontmatter or first line).
    pub description: String,
    /// Template body, with placeholders like `$1` already un-substituted.
    pub content: String,
    /// Source path (informational; used for diagnostic messages).
    pub path: PathBuf,
}

/// Warning emitted while loading prompt templates.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptTemplateDiagnostic {
    pub kind: DiagnosticKind,
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticKind {
    FileInfo,
    List,
    Read,
    Parse,
}

/// Frontmatter parse result: `Ok((meta, body))` or `Err(message)`.
///
/// - If the content does NOT start with `---`, the entire content is
///   the body and metadata is empty.
/// - If the content has `---` but no closing `---`, same: everything
///   is body, metadata is empty (matches Pi's behavior).
/// - Otherwise, YAML between the two `---` fences is parsed.
pub fn parse_frontmatter(content: &str) -> Result<(serde_yaml::Mapping, String), String> {
    // Normalize line endings (Pi does this too).
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.starts_with("---") {
        return Ok((serde_yaml::Mapping::new(), normalized));
    }
    // Find the closing `---` fence. Pi looks for "\n---" starting at byte 3.
    let end_marker = "\n---";
    let end_index = match normalized[3..].find(end_marker) {
        Some(i) => 3 + i,
        None => return Ok((serde_yaml::Mapping::new(), normalized)),
    };
    // Extract YAML between the two fences. Skip the opening `---` and
    // the following newline.
    let mut yaml_start = 3_usize;
    let yaml_end = end_index;
    if normalized[yaml_start..yaml_end].starts_with('\n') {
        yaml_start += 1;
    }
    let yaml_text = &normalized[yaml_start..yaml_end];
    let body_start = end_index + end_marker.len();
    let body = normalized[body_start..].trim().to_string();
    // Parse YAML.
    let yaml_value: serde_yaml::Value = match serde_yaml::from_str(yaml_text) {
        Ok(v) => v,
        Err(e) => return Err(format!("parse failed: {e}")),
    };
    let mapping = match yaml_value {
        serde_yaml::Value::Mapping(m) => m,
        _ => return Err("frontmatter is not a YAML mapping".to_string()),
    };
    Ok((mapping, body))
}

/// Result of loading prompt templates from a set of paths.
#[derive(Debug, Default, Clone)]
pub struct LoadResult {
    pub templates: Vec<PromptTemplate>,
    pub diagnostics: Vec<PromptTemplateDiagnostic>,
}

/// Default user-level prompt templates directory: `~/.pi/agent/prompts/`.
pub fn user_prompts_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".pi").join("agent").join("prompts"))
}

/// Default project-level prompt templates directory: `./.pi/prompts/`.
pub fn project_prompts_dir(cwd: &Path) -> Option<PathBuf> {
    Some(cwd.join(".pi").join("prompts"))
}

/// Load prompt templates from one or more paths (files or directories).
/// Mirrors Pi's `loadPromptTemplates(env, paths)`. Missing paths and
/// non-markdown files are skipped; read and parse failures are returned
/// as diagnostics (non-fatal).
pub fn load_prompt_templates<I, P>(paths: I) -> LoadResult
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut result = LoadResult::default();
    for path in paths {
        let path = path.as_ref();
        if !path.exists() {
            continue;
        }
        let md = match fs::metadata(path) {
            Ok(m) if m.is_dir() => {
                // Direct children only (non-recursive), like Pi.
                let entries = match fs::read_dir(path) {
                    Ok(e) => e,
                    Err(err) => {
                        result.diagnostics.push(PromptTemplateDiagnostic {
                            kind: DiagnosticKind::List,
                            path: path.to_path_buf(),
                            message: err.to_string(),
                        });
                        continue;
                    }
                };
                let mut paths = Vec::new();
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()).map_or(false, |e| e.eq_ignore_ascii_case("md")) {
                        paths.push(p);
                    }
                }
                paths
            }
            Ok(m) if m.is_file() => {
                if path.extension().and_then(|e| e.to_str()).map_or(false, |e| e.eq_ignore_ascii_case("md")) {
                    vec![path.to_path_buf()]
                } else {
                    continue;
                }
            }
            Ok(_) => continue,
            Err(err) => {
                result.diagnostics.push(PromptTemplateDiagnostic {
                    kind: DiagnosticKind::FileInfo,
                    path: path.to_path_buf(),
                    message: err.to_string(),
                });
                continue;
            }
        };
        for path in md {
            let raw = match fs::read_to_string(&path) {
                Ok(r) => r,
                Err(err) => {
                    result.diagnostics.push(PromptTemplateDiagnostic {
                        kind: DiagnosticKind::Read,
                        path: path.clone(),
                        message: err.to_string(),
                    });
                    continue;
                }
            };
            let (meta, body) = match parse_frontmatter(&raw) {
                Ok(p) => p,
                Err(err) => {
                    result.diagnostics.push(PromptTemplateDiagnostic {
                        kind: DiagnosticKind::Parse,
                        path: path.clone(),
                        message: err,
                    });
                    continue;
                }
            };
            // Template name from filename (without .md).
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("template")
                .to_string();
            // Description: explicit frontmatter `description`, else first
            // non-empty line of body, truncated to 60 chars (Pi's behavior).
            let description = match meta.get(&serde_yaml::Value::String("description".into())) {
                Some(serde_yaml::Value::String(s)) => s.clone(),
                _ => {
                    let first = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                    if first.len() > 60 {
                        format!("{}...", &first[..60])
                    } else {
                        first.to_string()
                    }
                }
            };
            result.templates.push(PromptTemplate {
                name,
                description,
                content: body,
                path: path.clone(),
            });
        }
    }
    result
}

/// Parse an argument string using shell-style single and double quotes.
/// Mirrors Pi's `parseCommandArgs`. Used by `/prompt <name> <args>` to
/// split positional arguments that fill `$1`, `$2`, ...
pub fn parse_command_args(args_string: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;
    let mut chars = args_string.chars().peekable();
    while let Some(c) = chars.next() {
        match (in_quote, c) {
            (Some(q), c) if c == q => in_quote = None,
            (Some(_), c) => current.push(c),
            (None, '"' | '\'') => in_quote = Some(c),
            (None, ' ' | '\t') if !current.is_empty() => {
                args.push(std::mem::take(&mut current));
            }
            (None, c) => current.push(c),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Substitute prompt template placeholders with command arguments.
///
/// Supported placeholders (matching Pi v0.85.1's `substituteArgs`):
/// - `$1`, `$2`, ... — positional argument (1-indexed)
/// - `$@` / `$ARGUMENTS` — all arguments, joined by spaces
///
/// Note: Pi also defines `${@:N}` and `${@:N:L}` for slicing arg
/// arrays. We omit them because the regex form `\$\{@:...}` requires
/// the literal escape sequences in the template body, which is
/// unfriendly for hand-written .md files. The three placeholders
/// above cover the documented Pi examples (`implement.md`,
/// `implement-and-review.md`). Implement `${@:N}` later if needed
/// by switching to `${...}` syntax without the backslash escapes.
pub fn substitute_args(content: &str, args: &[String]) -> String {
    use std::sync::OnceLock;
    static RE_DOLLAR_NUM: OnceLock<regex::Regex> = OnceLock::new();
    static RE_ARGUMENTS: OnceLock<regex::Regex> = OnceLock::new();
    static RE_AT: OnceLock<regex::Regex> = OnceLock::new();

    let re_dollar = RE_DOLLAR_NUM.get_or_init(|| regex::Regex::new(r"\$(\d+)").unwrap());
    let re_args = RE_ARGUMENTS
        .get_or_init(|| regex::Regex::new(r"\$ARGUMENTS").unwrap());
    let re_at = RE_AT.get_or_init(|| regex::Regex::new(r"\$@").unwrap());

    let mut out = content.to_string();
    // $N — positional (1-indexed).
    out = re_dollar
        .replace_all(&out, |caps: &regex::Captures<'_>| {
            let n: usize = caps[1].parse().unwrap_or(0);
            if n == 0 || n > args.len() {
                String::new()
            } else {
                args[n - 1].clone()
            }
        })
        .into_owned();
    let all_args = args.join(" ");
    out = re_args.replace_all(&out, all_args.as_str()).into_owned();
    out = re_at.replace_all(&out, all_args.as_str()).into_owned();
    out
}

/// Format a prompt template invocation with positional arguments.
/// Mirrors Pi's `formatPromptTemplateInvocation(template, args)`.
pub fn format_invocation(template: &PromptTemplate, args: &[String]) -> String {
    substitute_args(&template.content, args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_frontmatter_empty() {
        let (meta, body) = parse_frontmatter("hello world").unwrap();
        assert_eq!(meta.len(), 0);
        assert_eq!(body, "hello world");
    }

    #[test]
    fn parse_frontmatter_simple() {
        let raw = "---\ndescription: My template\n---\nbody content";
        let (meta, body) = parse_frontmatter(raw).unwrap();
        assert_eq!(
            meta.get(&serde_yaml::Value::String("description".into()))
                .and_then(|v| v.as_str()),
            Some("My template")
        );
        assert_eq!(body, "body content");
    }

    #[test]
    fn parse_frontmatter_unterminated() {
        // No closing --- → treat whole content as body.
        let raw = "---\ndescription: missing closer\nbody is here";
        let (meta, body) = parse_frontmatter(raw).unwrap();
        assert_eq!(meta.len(), 0);
        assert_eq!(body, raw);
    }

    #[test]
    fn parse_frontmatter_with_blank_line() {
        // Pi accepts optional leading newline after opening ---
        let raw = "---\n\ndescription: ok\n---\nbody";
        let (meta, body) = parse_frontmatter(raw).unwrap();
        assert_eq!(
            meta.get(&serde_yaml::Value::String("description".into()))
                .and_then(|v| v.as_str()),
            Some("ok")
        );
        assert_eq!(body, "body");
    }

    #[test]
    fn parse_frontmatter_complex_yaml() {
        let raw = r#"---
description: Complex
author: alice
tags:
  - rust
  - tui
---
# Body"#;
        let (meta, body) = parse_frontmatter(raw).unwrap();
        assert_eq!(
            meta.get(&serde_yaml::Value::String("author".into()))
                .and_then(|v| v.as_str()),
            Some("alice")
        );
        assert_eq!(
            meta.get(&serde_yaml::Value::String("tags".into()))
                .and_then(|v| v.as_sequence())
                .map(|s| s.len()),
            Some(2)
        );
        assert_eq!(body, "# Body");
    }

    #[test]
    fn parse_command_args_simple() {
        assert_eq!(
            parse_command_args("a b c"),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn parse_command_args_quoted() {
        assert_eq!(
            parse_command_args(r#"hello "world with spaces" end"#),
            vec![
                "hello".to_string(),
                "world with spaces".to_string(),
                "end".to_string()
            ]
        );
    }

    #[test]
    fn parse_command_args_single_quotes() {
        assert_eq!(
            parse_command_args("a 'b c' d"),
            vec!["a".to_string(), "b c".to_string(), "d".to_string()]
        );
    }

    #[test]
    fn substitute_args_positional() {
        let result = substitute_args("hello $1, you are $2", &["alice".into(), "lucky".into()]);
        assert_eq!(result, "hello alice, you are lucky");
    }

    #[test]
    fn substitute_args_at_and_arguments() {
        let result =
            substitute_args("[$@] [$ARGUMENTS]", &["a".into(), "b".into(), "c".into()]);
        assert_eq!(result, "[a b c] [a b c]");
    }

    #[test]
    fn substitute_args_at_range_omitted() {
        // `${@:N}` and `${@:N:L}` are deliberately not implemented in v1
        // because they require the user to write literal escape sequences
        // (`\$\{@:2\}`) in their .md files. The documented placeholders
        // (`$1`, `$@`, `$ARGUMENTS`) cover Pi's example templates.
        let result = substitute_args(
            "literal=$\\{unused\\}, plain=$@",
            &["alpha".into(), "beta".into()],
        );
        assert_eq!(result, "literal=$\\{unused\\}, plain=alpha beta");
    }

    #[test]
    fn substitute_args_missing_position() {
        let result = substitute_args("missing $5", &["a".into()]);
        assert_eq!(result, "missing ");
    }

    #[test]
    fn load_prompt_templates_from_directory() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("explain.md"),
            "---\ndescription: Explain code\n---\nExplain this:\n$1",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("test.md"),
            "---\ndescription: Write tests\n---\nWrite tests for:\n$1",
        )
        .unwrap();
        std::fs::write(tmp.path().join("ignored.txt"), "not markdown").unwrap();
        let result = load_prompt_templates([tmp.path()]);
        assert_eq!(result.templates.len(), 2);
        assert!(result.diagnostics.is_empty());
        let names: Vec<_> = result.templates.iter().map(|t| &t.name).collect();
        assert!(names.contains(&&"explain".to_string()));
        assert!(names.contains(&&"test".to_string()));
    }

    #[test]
    fn load_prompt_templates_first_line_fallback_description() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("no_front.md"),
            "This is the first line that should become the description.",
        )
        .unwrap();
        let result = load_prompt_templates([tmp.path()]);
        assert_eq!(result.templates.len(), 1);
        assert_eq!(
            result.templates[0].description,
            "This is the first line that should become the description."
        );
    }

    #[test]
    fn load_prompt_templates_truncates_long_description() {
        let tmp = TempDir::new().unwrap();
        let long = "x".repeat(100);
        std::fs::write(tmp.path().join("long.md"), long.clone()).unwrap();
        let result = load_prompt_templates([tmp.path()]);
        assert_eq!(result.templates.len(), 1);
        // 60 chars + "..." = 63
        assert_eq!(result.templates[0].description.len(), 63);
        assert!(result.templates[0].description.ends_with("..."));
    }

    #[test]
    fn load_prompt_templates_missing_path_skipped() {
        let result = load_prompt_templates([std::path::Path::new("/nonexistent/path")]);
        assert!(result.templates.is_empty());
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn load_prompt_templates_parse_failure_emits_diagnostic() {
        // Invalid YAML should produce a diagnostic, not panic.
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("bad.md"),
            "---\n: invalid: yaml: : :\n---\nbody",
        )
        .unwrap();
        let result = load_prompt_templates([tmp.path()]);
        assert!(result.templates.is_empty());
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].kind, DiagnosticKind::Parse);
    }

    #[test]
    fn load_prompt_templates_non_md_file_skipped() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("README.txt"), "not a template").unwrap();
        std::fs::write(
            tmp.path().join("actual.md"),
            "---\ndescription: Real\n---\nuses $1",
        )
        .unwrap();
        let result = load_prompt_templates([tmp.path()]);
        assert_eq!(result.templates.len(), 1);
        assert_eq!(result.templates[0].name, "actual");
    }

    #[test]
    fn load_prompt_templates_from_specific_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("single.md");
        std::fs::write(&path, "---\ndescription: Single\n---\nbody $1").unwrap();
        let result = load_prompt_templates([&path]);
        assert_eq!(result.templates.len(), 1);
        assert_eq!(result.templates[0].name, "single");
        assert_eq!(result.templates[0].content, "body $1");
    }

    #[test]
    fn format_invocation_substitutes_placeholders() {
        let template = PromptTemplate {
            name: "greet".to_string(),
            description: "Greeting".to_string(),
            content: "Hello $1, welcome to $2!".to_string(),
            path: std::path::PathBuf::from("/dev/null"),
        };
        let result =
            format_invocation(&template, &["Alice".to_string(), "Wonderland".to_string()]);
        assert_eq!(result, "Hello Alice, welcome to Wonderland!");
    }

    #[test]
    fn user_prompts_dir_resolves_under_home() {
        // Not portable across environments, but we can at least assert
        // that the function returns Some path when HOME is set.
        std::env::set_var("HOME", "/tmp/test-home");
        assert_eq!(
            user_prompts_dir(),
            Some(std::path::PathBuf::from("/tmp/test-home/.pi/agent/prompts"))
        );
    }

    #[test]
    fn project_prompts_dir_under_cwd() {
        let dir = project_prompts_dir(std::path::Path::new("/some/cwd"));
        assert_eq!(
            dir,
            Some(std::path::PathBuf::from("/some/cwd/.pi/prompts"))
        );
    }
}
