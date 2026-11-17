//! Quality-left-shift constraint checks (phase-1 PoC).
//!
//! Maps the quality-left-shift scheme (concept page
//! `concepts/agent-quality-engineering-constraint-2026-11.md`, deep-dive II)
//! into executable static rules. A static check must run **before** a bad
//! change enters the mainline, so the rules operate on the **added lines**
//! of a change (a diff), not on whole files.
//!
//! Rule mapping (Rust-agent structural equivalents — the source video's
//! React-specific semantics map to their structural counterpart here):
//! - R1 `render-layer side-effect hook` → flag direct side-effectful calls
//!   in agent/tool hot-path files (side effects must be closed in a
//!   controlled layer).
//! - R2 `cross-dir / backend cross-reference` → flag added module/import
//!   paths crossing a configured non-business-directory boundary.
//! - R3 `high comment ratio` → flag changed files whose added lines are
//!   dominated by comment-only lines (stale comments are a form of an
//!   "already handled" excuse).
//! - R4 `timer patch` → flag added business-source lines calling
//!   `setTimeout`/`setInterval` for patching (hides root cause).
//!
//! Honesty: these are structural PoC checks; the React `useEffect` semantics
//! are mapped to their structural equivalent in the Rust agent. They do not
//! replace runtime verification or formal verification.

use regex::Regex;

/// Configuration for the PoC rule set.
pub struct ConstraintConfig {
    /// R3 threshold: max comment-only share of added lines in a file
    /// (0.0..1.0).
    pub comment_ratio_threshold: f64,
    /// R1: files/globs treated as agent hot path (side effects must be
    /// closed in a controlled layer).
    pub hot_path_globs: [&'static str; 2],
    /// R2: patterns for backend / non-business directories.
    pub non_business_dir_patterns: [&'static str; 2],
}

impl Default for ConstraintConfig {
    fn default() -> Self {
        ConstraintConfig {
            comment_ratio_threshold: 0.5,
            hot_path_globs: [
                "crates/nini-core/src/agent.rs",
                "crates/nini-core/src/agent_hooks.rs",
            ],
            non_business_dir_patterns: ["backend", "nini-nonbiz"],
        }
    }
}

/// R1 rule id.
pub const R1: &str = "R1";
/// R2 rule id.
pub const R2: &str = "R2";
/// R3 rule id.
pub const R3: &str = "R3";
/// R4 rule id.
pub const R4: &str = "R4";

/// A single constraint violation, as an owned record.
pub struct Violation {
    pub rule: &'static str,
    pub file: String,
    pub line: Option<usize>,
    pub reason: String,
}

fn is_comment_line(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with("//") || t.starts_with('*') || t.starts_with("/*")
}

/// Added lines of a change (the diff surface the rules inspect).
pub fn added_lines_of(content: &str) -> Vec<String> {
    content.lines().map(str::to_string).collect()
}

fn path_matches_any(path: &str, patterns: &[&str]) -> bool {
    for p in patterns {
        if path.contains(p) {
            return true;
        }
    }
    false
}

/// Run the R1–R4 constraints over the added lines of each change file.
///
/// Returns one owned record per violation (rule id, file, line, reason).
pub fn check_change(files: &[(String, Vec<String>)], config: &ConstraintConfig) -> Vec<String> {
    let hot_path =
        Regex::new(r"fs::(write\(|create\(|remove\(|truncate\(|rename\(|create_dir\(|write_all\()")
            .unwrap();
    let module_or_use = Regex::new(r"^(mod |pub\s+mod |use |pub\s+use )").unwrap();
    let timer = Regex::new(r"setInterval\(|setTimeout\(").unwrap();

    let mut violations = Vec::new();
    for (file, lines) in files {
        if lines.is_empty() {
            continue;
        }
        // R3: comment-only share of added lines.
        let total = lines.len();
        let comments = lines
            .iter()
            .filter(|l: &&String| is_comment_line(*l))
            .count() as f64;
        if comments as f64 / total as f64 > config.comment_ratio_threshold {
            violations.push(format!(
                "R3 {}: comment-only share {:.0}% exceeds threshold {:.0}%",
                file,
                (comments as f64 / total as f64 * 100.0) as i64,
                config.comment_ratio_threshold * 100.0
            ));
        }
        for (i, line) in lines.iter().enumerate() {
            let line_no = i + 1;
            // R1: hot-path direct side-effect call.
            if path_matches_any(file, &config.hot_path_globs) && hot_path.is_match(line) {
                violations.push(format!(
                    "R1 {}:{} hot-path direct side-effect call (side effects must be closed in a controlled layer)",
                    file, line_no
                ));
            }
            // R2: module/import crossing a non-business-directory boundary.
            if module_or_use.is_match(line) {
                let rest = line.splitn(2, char::is_whitespace).nth(1).unwrap_or("");
                for p in &config.non_business_dir_patterns {
                    if rest.contains(p) {
                        violations.push(format!(
                            "R2 {}:{} cross-dir/backend reference ({} outside business dir boundary)",
                            file, line_no, p
                        ));
                        break;
                    }
                }
            }
            // R4: timer patch in business code.
            if timer.is_match(line) {
                violations.push(format!(
                    "R4 {}:{} timer patch (setTimeout/setInterval) — must fix root cause",
                    file, line_no
                ));
            }
        }
    }
    violations.sort();
    violations
}

/// Convenience: true if any violation exists for the change.
pub fn check_has_violations(files: &[(String, Vec<String>)], config: &ConstraintConfig) -> bool {
    !check_change(files, config).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ConstraintConfig {
        ConstraintConfig::default()
    }

    #[test]
    fn r4_flags_timer_patch() {
        let files = vec![(
            "crates/nini-tools/src/edit.rs".into(),
            vec!["let t = setInterval(() => {}, 1000);".into()],
        )];
        let v = check_change(&files, &cfg());
        assert!(v.iter().any(|x: &String| x.contains("R4")), "got {v:?}");
    }

    #[test]
    fn r3_flags_comment_dominated_added_lines() {
        let files = vec![(
            "crates/nini-tools/src/edit.rs".into(),
            vec![
                "let a = 1;".into(),
                "// stale comment already handled".into(),
                "// another stale comment".into(),
            ],
        )];
        let v = check_change(&files, &cfg());
        assert!(v.iter().any(|x: &String| x.contains("R3")), "got {v:?}");
    }

    #[test]
    fn r1_flags_hot_path_side_effect() {
        let files = vec![(
            "crates/nini-core/src/agent.rs".into(),
            vec!["std::fs::write(&tmp, b\"x\").await;".into()],
        )];
        let v = check_change(&files, &cfg());
        assert!(v.iter().any(|x: &String| x.contains("R1")), "got {v:?}");
    }

    #[test]
    fn r2_flags_background_cross_reference() {
        let files = vec![(
            "crates/nini-tools/src/edit.rs".into(),
            vec!["use crate::backend::common::x;".into()],
        )];
        let v = check_change(&files, &cfg());
        assert!(v.iter().any(|x: &String| x.contains("R2")), "got {v:?}");
    }

    #[test]
    fn clean_change_passes() {
        let files = vec![(
            "crates/nini-tools/src/write.rs".into(),
            vec![
                "let tmp = path.clone();".into(),
                "fs::write(&tmp, data).await?;".into(),
            ],
        )];
        assert!(
            check_change(&files, &cfg()).is_empty(),
            "clean change must not violate"
        );
    }

    #[test]
    fn config_threshold_is_configurable() {
        let config = ConstraintConfig {
            comment_ratio_threshold: 1.0, // never flag comments
            ..cfg()
        };
        let files = vec![(
            "crates/nini-tools/src/edit.rs".into(),
            vec!["// only a comment".into()],
        )];
        assert!(check_change(&files, &config).is_empty());
    }
}
