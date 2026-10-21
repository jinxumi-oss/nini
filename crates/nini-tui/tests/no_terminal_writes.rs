//! Nothing may write to the terminal while the TUI owns it.
//! invariant: ca97731b
//!
//! The TUI paints the whole screen, and the library writes to stderr:
//! `nini_core::project_trust::check` prints a trust warning on **every
//! tool call**, the agent prints on its error paths, and the TUI itself
//! printed in half a dozen places. Every one of those writes landed on
//! the alternate screen and interleaved with the repaint — the user read
//! two texts fighting over one row.
//!
//! The runtime now has a choke point (`terminal_guard`): while the TUI
//! runs, fd 2 is a pipe, and the lines in it become transcript notices.
//! That fixes the *symptom* everywhere. This test fixes the *recurrence*
//! where it can be fixed: a new `eprintln!` in the TUI's own source, or
//! in the library it drives, is a write to a screen nobody owns, and it
//! has to be a deliberate decision (an entry in the whitelist below with
//! a reason), not an accident.
//!
//! The whitelist is deliberately narrow, and every entry says why it is
//! safe. A file that is not on the list must have **no** terminal write
//! at all.

use std::path::{Path, PathBuf};

/// Files that may still write to the terminal: `(path, expected count,
/// why it is safe)`.
///
/// The count is part of the entry on purpose: a line number would move
/// with every edit, but a *new* write in a whitelisted file has to
/// break this test, so the count is what is pinned.
const WHITELIST: &[(&str, usize, &str)] = &[
    (
        "crates/nini-tui/src/theme.rs",
        3,
        "startup: the theme is resolved before the alternate screen is \
         entered, so there is no painted frame to corrupt",
    ),
    (
        "crates/nini-tui/src/signals.rs",
        4,
        "signal setup and the panic hook: the setup runs before the TUI \
         owns the screen, and the panic hook calls \
         `terminal_guard::release_stderr` before it writes, so a crash \
         report still reaches the terminal",
    ),
    (
        "crates/nini-tui/src/keybindings_manager.rs",
        4,
        "diagnostics for a keybindings file. The TUI runtime never calls \
         this entry point (it uses `load_user_overrides`, which is \
         silent), and if it ever did, the guard would catch it",
    ),
    (
        "crates/nini-core/src/agent.rs",
        6,
        "library diagnostics. While the TUI runs, the terminal guard \
         catches them and they become transcript notices; outside the \
         TUI, stderr is the only channel the library has",
    ),
    (
        "crates/nini-core/src/agent_hooks.rs",
        2,
        "library diagnostics — see the note on `nini-core/src/agent.rs`",
    ),
    (
        "crates/nini-core/src/trust_gate.rs",
        1,
        "library diagnostics — see the note on `nini-core/src/agent.rs`",
    ),
    (
        "crates/nini-core/src/compaction.rs",
        1,
        "library diagnostics — see the note on `nini-core/src/agent.rs`",
    ),
    (
        "crates/nini-core/src/project_trust.rs",
        1,
        "the trust warning that produced the garbled screen. The guard \
         catches it while the TUI runs — see the note on \
         `nini-core/src/agent.rs`",
    ),
];

/// The crates whose sources are checked.
const CRATES: &[&str] = &["crates/nini-tui/src", "crates/nini-core/src"];

/// The macros that write to the terminal.
const PRINT_MACROS: &[&str] = &["print!", "println!", "eprint!", "eprintln!"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .to_path_buf()
}

/// Blank out everything that is not code: comments and string literals.
///
/// A `println!` inside a markdown sample is not a write to the
/// terminal, and neither is one inside a doc comment.
fn code_only(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c == '/' && i + 1 < n && chars[i + 1] == '/' {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < n && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(n);
            out.push(' ');
            continue;
        }
        // Raw string: `r"…"`, `r#"…"#`.
        if c == 'r' && i + 1 < n && (chars[i + 1] == '"' || chars[i + 1] == '#') {
            let mut hashes = 0usize;
            let mut j = i + 1;
            while j < n && chars[j] == '#' {
                hashes += 1;
                j += 1;
            }
            if j < n && chars[j] == '"' {
                j += 1;
                while j < n {
                    if chars[j] == '"' {
                        let mut k = j + 1;
                        let mut seen = 0usize;
                        while k < n && chars[k] == '#' && seen < hashes {
                            seen += 1;
                            k += 1;
                        }
                        if seen == hashes {
                            j = k;
                            break;
                        }
                    }
                    j += 1;
                }
                i = j;
                out.push('"');
                continue;
            }
        }
        if c == '"' {
            i += 1;
            while i < n {
                if chars[i] == '\\' {
                    i += 2;
                    continue;
                }
                if chars[i] == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push('"');
            continue;
        }
        // Char literal: `'x'` or `'\n'`. A lifetime (`'a`) is not one.
        if c == '\'' {
            if i + 2 < n && chars[i + 2] == '\'' && chars[i + 1] != '\\' {
                i += 3;
                out.push('\'');
                continue;
            }
            if i + 3 < n && chars[i + 1] == '\\' && chars[i + 3] == '\'' {
                i += 4;
                out.push('\'');
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Blank out the `#[cfg(test)]` modules.
///
/// A test's `eprintln!` is diagnostics for the test runner, not a
/// write to a screen the TUI owns.
fn strip_cfg_test(src: &str) -> String {
    let mut out = src.to_string();
    let mut from = 0usize;
    while let Some(pos) = out[from..].find("#[cfg(test)]") {
        let start = from + pos;
        let Some(rel) = out[start..].find('{') else {
            break;
        };
        let brace = start + rel;
        let mut depth = 0usize;
        let mut end = out.len();
        for (i, ch) in out[brace..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = brace + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        // Keep the newlines, so the line numbers stay right.
        let blanked: String = out[start..end]
            .chars()
            .map(|c| if c == '\n' { '\n' } else { ' ' })
            .collect();
        out.replace_range(start..end, &blanked);
        from = end;
    }
    out
}

/// The line numbers of the terminal writes in one source file.
fn print_sites(src: &str) -> Vec<usize> {
    let code = strip_cfg_test(&code_only(src));
    let mut sites = Vec::new();
    for macro_name in PRINT_MACROS {
        let mut from = 0usize;
        while let Some(pos) = code[from..].find(macro_name) {
            let at = from + pos;
            from = at + macro_name.len();
            // Not a macro call if it is part of a longer identifier.
            let before = code[..at].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let after = code[from..].chars().next();
            if after.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            sites.push(code[..at].matches('\n').count() + 1);
        }
    }
    sites.sort_unstable();
    sites.dedup();
    sites
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rust_files(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}

#[test]
fn no_source_file_writes_to_the_terminal_outside_the_whitelist() {
    let root = repo_root();
    let mut problems: Vec<String> = Vec::new();

    for crate_dir in CRATES {
        let dir = root.join(crate_dir);
        assert!(dir.is_dir(), "missing source directory: {}", dir.display());
        for path in rust_files(&dir) {
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let src = std::fs::read_to_string(&path).expect("read source");
            let sites = print_sites(&src);
            if sites.is_empty() {
                continue;
            }
            match WHITELIST.iter().find(|(w, _, _)| *w == rel) {
                Some((_, expected, reason)) => {
                    if sites.len() != *expected {
                        problems.push(format!(
                            "{rel}: {} terminal write(s) (lines {}), the whitelist says {expected}. \
                             A new write here needs a decision, not a bigger number — {reason}",
                            sites.len(),
                            sites
                                .iter()
                                .map(|l| l.to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                }
                None => problems.push(format!(
                    "{rel}: writes to the terminal on line(s) {}. Use the notice channel \
                     (`AppState::push_notice`) or add a whitelist entry with a reason.",
                    sites
                        .iter()
                        .map(|l| l.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }
    }

    assert!(
        problems.is_empty(),
        "these writes go to a screen the TUI is painting:\n{}",
        problems.join("\n")
    );
}

#[test]
fn every_whitelisted_file_still_exists() {
    // A whitelist entry for a file that no longer exists is a stale
    // exemption: it would silently stop being checked.
    let root = repo_root();
    for (rel, _, _) in WHITELIST {
        assert!(
            root.join(rel).is_file(),
            "the whitelist names {rel}, which does not exist"
        );
    }
}
