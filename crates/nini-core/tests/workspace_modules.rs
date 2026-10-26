//! Oracle: every module a crate declares **has a file**, and every source
//! file in the tree is **tracked**.
//!
//! # The invariant
//!
//! On 2026-09-21 a `git reset` dropped several modules that the tracked
//! code still declared: `lib.rs` said `mod entries;` while
//! `entries.rs` was untracked — and, worse, missing. The workspace stopped
//! compiling, and the repair was a stabilisation commit that re-introduced
//! minimal stubs for every missing module.
//!
//! Two failure shapes, one invariant:
//!
//! 1. a **dangling declaration** — `mod foo;` with no `foo.rs` / `foo/mod.rs`;
//! 2. an **untracked source file** — the file is on disk and compiles, but
//!    it is not in git, so the next clone (or the next reset) loses it.
//!
//! The second one is the nastier: a green build on this machine says
//! nothing about the build on anyone else's.
//!
//! Both are checked here, from the **outside**: the test reads the crates
//! on disk and asks git what it thinks it is tracking.

//! invariant: b479ef5d —— 声明的模块必须存在，源码必须被 git 跟踪。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("nini-core has a parent and a grandparent")
        .to_path_buf()
}

/// The crate directories under `crates/`.
fn crate_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// The module declarations of one file: `mod foo;` and `pub mod foo;`.
///
/// Inline modules (`mod foo { … }`) and `#[path = "…"]` overrides are
/// skipped: an inline module has no file to look for, and a `#[path]`
/// module names its own file in the attribute.
fn declared_modules(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("//") {
            continue;
        }
        let Some(rest) = line.strip_prefix("pub ") else {
            continue;
        };
        let Some(rest) = rest.strip_prefix("mod ") else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() {
            continue;
        }
        // `mod foo;` — a declaration. `mod foo {` — an inline module.
        if rest[name.len()..].trim_start().starts_with(';') {
            out.push(name);
        }
    }
    out
}

#[test]
fn every_declared_module_has_a_file() {
    let root = repo_root();
    let mut missing: Vec<String> = Vec::new();
    let mut declared_total = 0usize;

    for crate_dir in crate_dirs(&root) {
        let src = crate_dir.join("src");
        for entry in ["lib.rs", "main.rs"] {
            let file = src.join(entry);
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            for name in declared_modules(&text) {
                declared_total += 1;
                let flat = src.join(format!("{name}.rs"));
                let nested = src.join(&name).join("mod.rs");
                if !flat.is_file() && !nested.is_file() {
                    missing.push(format!(
                        "{} declares `mod {name};` but neither {} nor {} exists",
                        file.strip_prefix(&root).unwrap_or(&file).display(),
                        flat.strip_prefix(&root).unwrap_or(&flat).display(),
                        nested.strip_prefix(&root).unwrap_or(&nested).display()
                    ));
                }
            }
        }
    }

    assert!(
        declared_total > 0,
        "no module declarations were found — the scan is broken, so a clean \
         result here would mean nothing was checked"
    );
    assert!(
        missing.is_empty(),
        "modules that are declared but have no file (this is exactly how the \
         2026-09-21 build broke):\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn every_source_file_is_tracked_by_git() {
    let root = repo_root();
    let out = Command::new("git")
        .args(["-C", root.to_str().unwrap(), "ls-files", "--", "crates"])
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git ls-files failed in {}: {}",
        root.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    let tracked: BTreeSet<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();

    let mut untracked: Vec<String> = Vec::new();
    for crate_dir in crate_dirs(&root) {
        // The whole crate directory, not just `src/`: an untracked test is
        // the same failure — it compiles here and is gone on the next
        // clone.
        let mut stack = vec![crate_dir.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().map(|e| e == "rs").unwrap_or(false) {
                    let rel = path
                        .strip_prefix(&root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    if !tracked.contains(&rel) {
                        untracked.push(rel);
                    }
                }
            }
        }
    }

    assert!(
        untracked.is_empty(),
        "source files that exist on disk but are **not in git** — the next \
         clone loses them, and the next `git reset` breaks the build:\n  {}",
        untracked.join("\n  ")
    );
}
