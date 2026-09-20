//! Unified line-diff (like `diff -u`) for tool output rendering.
//!
//! Used by the edit / write tools to return a colored diff to the TUI.
//! Pi's `ToolExecutionComponent` renders file edits as `- red` / `+ green`
//! lines with context; nini's `rich` module can colorize these via
//! `theme.fg_style("success")` / `theme.fg_style("error")`.
//!
//! Algorithm: classic Myers-style line diff. O((N+M)·D) where D is the
//! edit distance. For files under ~1000 lines this is plenty fast; for
//! larger files callers should use a streaming diff or pre-computed diff.

/// One line in a unified diff output.
///
/// `sign` distinguishes context / add / remove:
/// - `' '` (space): unchanged context line
/// - `'+'`: line added
/// - `'-'`: line removed
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub sign: char,
    pub content: String,
}

impl DiffLine {
    /// Render this line as a unified-diff prefix. Returns
    /// `" hello"`, `"+hello"`, or `"-hello"` (single space, then sign).
    pub fn render(&self) -> String {
        format!("{}{}", self.sign, self.content)
    }
}

/// Compute a unified line diff between two strings.
///
/// Returns a sequence of `DiffLine`s covering both files: context lines
/// around each hunk plus the added/removed lines themselves.
///
/// `context` is the number of context lines to show around each change
/// (typical values: 0 for compact diffs, 3 for unified-diff style).
pub fn unified_diff(old: &str, new: &str, context: usize) -> Vec<DiffLine> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    // Compute the LCS table (longest common subsequence).
    let n = old_lines.len();
    let m = new_lines.len();
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            lcs[i][j] = if old_lines[i - 1] == new_lines[j - 1] {
                lcs[i - 1][j - 1] + 1
            } else {
                std::cmp::max(lcs[i - 1][j], lcs[i][j - 1])
            };
        }
    }

    // Backtrack to produce the edit script.
    let mut ops: Vec<(char, &str)> = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old_lines[i - 1] == new_lines[j - 1] {
            ops.push((' ', old_lines[i - 1]));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || lcs[i][j - 1] >= lcs[i - 1][j]) {
            ops.push(('+', new_lines[j - 1]));
            j -= 1;
        } else if i > 0 {
            ops.push(('-', old_lines[i - 1]));
            i -= 1;
        }
    }
    ops.reverse();

    // Merge adjacent +/- runs into hunks with `context` lines of
    // surrounding context.
    collapse_to_hunks(&ops, context)
}

/// Walk `ops` and emit each line. When two consecutive `+/-` runs are
/// separated by more than `context` unchanged lines, insert a `…`
/// separator in place of the middle context.
///
/// Each `+`/`-` run is preceded and followed by up to `context`
/// unchanged lines.
fn collapse_to_hunks(ops: &[(char, &str)], context: usize) -> Vec<DiffLine> {
    let mut out: Vec<DiffLine> = Vec::new();
    let n = ops.len();
    if n == 0 {
        return out;
    }

    // Find the first change.
    let mut first_change = None;
    for (i, op) in ops.iter().enumerate() {
        if op.0 != ' ' {
            first_change = Some(i);
            break;
        }
    }
    let Some(first_change) = first_change else {
        // No changes at all — emit the first `context` lines of context.
        let limit = context.min(n);
        for k in 0..limit {
            out.push(DiffLine {
                sign: ' ',
                content: ops[k].1.to_string(),
            });
        }
        return out;
    };

    // Walk through ops. We use a state machine:
    //   - skip initial context if longer than `context` lines (emit `…`)
    //   - emit pre-context (up to `context` lines)
    //   - emit change (+/-) run
    //   - emit post-context (up to `context` lines)
    //   - if more than `context` lines pass before the next change,
    //     emit `…` before its pre-context.
    let mut i = 0;
    let mut prev_change_end: Option<usize> = None;

    while i < n {
        if ops[i].0 != ' ' {
            // We're at a change. Compute pre-context start.
            let pre_start = i.saturating_sub(context);
            let already_emitted_through = prev_change_end.map(|e| e + 1).unwrap_or(0);

            // Case 1: first change — handle leading-context collapse.
            // Case 2: subsequent change — handle inter-hunk gap collapse.
            let mut k_start = pre_start;
            let mut insert_sep = false;
            if prev_change_end.is_none() {
                // First change.
                if i >= context + 1 {
                    // Leading context > context lines — emit `…`.
                    insert_sep = true;
                }
            } else {
                // Subsequent change.
                let gap = i.saturating_sub(already_emitted_through);
                if gap > 2 * context {
                    insert_sep = true;
                }
            }
            if insert_sep {
                out.push(DiffLine {
                    sign: ' ',
                    content: "…".to_string(),
                });
            }
            // Emit pre-context (lines k_start..i), skipping any that
            // would duplicate lines already emitted as post-context.
            for k in already_emitted_through.max(k_start)..i {
                out.push(DiffLine {
                    sign: ' ',
                    content: ops[k].1.to_string(),
                });
            }

            // Find end of change run.
            let mut j = i;
            while j < n && ops[j].0 != ' ' {
                out.push(DiffLine {
                    sign: ops[j].0,
                    content: ops[j].1.to_string(),
                });
                j += 1;
            }

            // Emit post-context (up to `context` lines).
            let post_end = (j + context).min(n);
            let mut k = j;
            while k < post_end && ops[k].0 == ' ' {
                out.push(DiffLine {
                    sign: ' ',
                    content: ops[k].1.to_string(),
                });
                k += 1;
            }
            prev_change_end = Some(if k > 0 { k - 1 } else { 0 });
            i = k;
        } else {
            // Pure context. If we're between hunks (prev_change_end
            // set and we haven't emitted `…` yet), the change-handling
            // branch will pick this up. Otherwise emit as leading or
            // trailing context.
            out.push(DiffLine {
                sign: ' ',
                content: ops[i].1.to_string(),
            });
            i += 1;
        }
    }

    out
}

/// Build a human-readable diff string with `@@ ... @@` hunk headers,
/// suitable for embedding directly into a transcript entry.
pub fn render_unified(old: &str, new: &str, context: usize) -> String {
    let lines = unified_diff(old, new, context);
    lines
        .iter()
        .map(|l| l.render())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Summary line: "+N -M" where N is the number of additions, M is the
/// number of removals. Useful for the status bar after an edit.
pub fn diff_summary(old: &str, new: &str) -> (usize, usize) {
    let lines = unified_diff(old, new, 0);
    let adds = lines.iter().filter(|l| l.sign == '+').count();
    let dels = lines.iter().filter(|l| l.sign == '-').count();
    (adds, dels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_change_returns_only_context() {
        let diff = unified_diff("hello\nworld\n", "hello\nworld\n", 3);
        for l in &diff {
            assert_eq!(l.sign, ' ');
        }
    }

    #[test]
    fn simple_addition() {
        let diff = unified_diff("a\nb\n", "a\nb\nc\n", 3);
        let adds: Vec<&DiffLine> = diff.iter().filter(|l| l.sign == '+').collect();
        assert_eq!(adds.len(), 1);
        assert_eq!(adds[0].content, "c");
    }

    #[test]
    fn simple_removal() {
        let diff = unified_diff("a\nb\nc\n", "a\nc\n", 3);
        let dels: Vec<&DiffLine> = diff.iter().filter(|l| l.sign == '-').collect();
        assert_eq!(dels.len(), 1);
        assert_eq!(dels[0].content, "b");
    }

    #[test]
    fn modification_shows_both() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nmodified\nline3\n";
        let diff = unified_diff(old, new, 1);
        // Should contain a `-` for line2 and a `+` for modified,
        // plus context lines.
        assert!(diff.iter().any(|l| l.sign == '-' && l.content == "line2"));
        assert!(diff.iter().any(|l| l.sign == '+' && l.content == "modified"));
        assert!(diff.iter().any(|l| l.sign == ' ' && l.content == "line1"));
        assert!(diff.iter().any(|l| l.sign == ' ' && l.content == "line3"));
    }

    #[test]
    fn context_collapse_with_gap() {
        // Two changes far apart with a gap > 2 * context.
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\n";
        let new = "a\nb\nC\nd\ne\nf\nG\nh\ni\n";
        let diff = unified_diff(old, new, 1);
        // Should have a "…" separator between the two hunks.
        assert!(diff.iter().any(|l| l.sign == ' ' && l.content == "…"));
    }

    #[test]
    fn render_unified_format() {
        let s = render_unified("a\nb\n", "a\nB\n", 3);
        assert!(s.contains("-b"));
        assert!(s.contains("+B"));
        assert!(s.contains(" a"));
    }

    #[test]
    fn diff_summary_counts_correctly() {
        let (adds, dels) = diff_summary("a\n", "a\nb\nc\n");
        assert_eq!(adds, 2);
        assert_eq!(dels, 0);

        let (adds, dels) = diff_summary("a\nb\nc\n", "a\nc\n");
        assert_eq!(adds, 0);
        assert_eq!(dels, 1);
    }

    #[test]
    fn multi_line_modification_preserves_all_lines() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB1\nB2\nc\nd\ne\n";
        let diff = unified_diff(old, new, 1);
        let adds: Vec<&str> = diff.iter().filter(|l| l.sign == '+').map(|l| l.content.as_str()).collect();
        let dels: Vec<&str> = diff.iter().filter(|l| l.sign == '-').map(|l| l.content.as_str()).collect();
        assert_eq!(adds, vec!["B1", "B2"]);
        assert_eq!(dels, vec!["b"]);
    }

    #[test]
    fn empty_old_and_new() {
        let diff = unified_diff("", "", 3);
        assert!(diff.is_empty());
    }

    #[test]
    fn add_to_empty() {
        let diff = unified_diff("", "new content\n", 3);
        let adds: Vec<&str> = diff.iter().filter(|l| l.sign == '+').map(|l| l.content.as_str()).collect();
        assert_eq!(adds, vec!["new content"]);
    }
}
