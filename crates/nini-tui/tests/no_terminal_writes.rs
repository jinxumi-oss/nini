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
//! where it can be fixed: a new terminal write in the TUI's own source,
//! or in the library it drives, has to be a deliberate decision (an entry
//! in a whitelist below with a reason), not an accident.
//!
//! # stdout and stderr are not the same thing
//!
//! The guard captures **stderr**. stdout is the terminal the renderer
//! paints on, and it cannot be captured — so a stdout write during a
//! TUI session is the original defect, unfixed, and it is *not* visible
//! to a test that only greps for `eprintln!`.
//!
//! That gap was measured: turning one `eprintln!` into a `println!` in
//! a whitelisted file left this test green, and the bug it was written
//! to prevent was back. So the two streams are judged separately:
//!
//! * **stdout** — a violation everywhere except the one-shot commands
//!   (`nini info`, `nini demo`, `--list-models`), which never run the
//!   TUI. No count is pinned for them beyond the file being named:
//!   their output *is* the product.
//! * **stderr** — allowed in the files listed below, with a pinned count
//!   each, because the guard turns them into transcript notices while
//!   the TUI runs (and outside the TUI, stderr is the only channel a
//!   library has).
//!
//! A whitelist entry that has gone stale — the file no longer writes, or
//! no longer exists — is a silent exemption, and is reported as one.

use std::path::{Path, PathBuf};

/// Files that may write to **stdout**, with the count and the reason.
///
/// stdout is the screen the TUI paints. The guard cannot capture it
/// (the renderer owns fd 1), so a stdout write while the TUI runs is
/// the original defect. These are the one-shot commands: they print
/// their result and exit, and they never open a TUI.
const STDOUT_WHITELIST: &[(&str, usize, &str)] = &[
    (
        "crates/nini-cli/src/demo.rs",
        1,
        "`nini demo` is a one-shot command: it runs a scripted task, prints the \
         result and exits. It never opens a TUI, so there is no painted frame \
         to corrupt",
    ),
    (
        "crates/nini-cli/src/main.rs",
        1,
        "`--list-models` and the usage text: printed before anything else runs, \
         and the TUI is not started on those paths",
    ),
    (
        "crates/nini-cli/src/info.rs",
        10,
        "`nini info` is a one-shot command: its output *is* the product, and it \
         never opens a TUI",
    ),
];

/// Files that may write to **stderr**, with the count and the reason.
///
/// While the TUI runs these are captured by the guard and become
/// transcript notices. The count is pinned: a *new* write in a
/// whitelisted file has to break this test, so an exemption has to be
/// re-decided rather than silently widened.
const STDERR_WHITELIST: &[(&str, usize, &str)] = &[
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
        2,
        "library diagnostics — see the note on `nini-core/src/agent.rs`. \
         The second one is the trust warning's default printer: it is a \
         function so that a test can hold the print still and check that \
         the trust store's lock is *not* held while it happens (a blocked \
         print under the lock would stall every other tool call)",
    ),
    // `crates/nini-core/src/project_trust.rs` used to be here: the trust
    // warning that produced the garbled screen was printed from
    // `ProjectTrustStore::check`. That print moved into the trust gate's
    // printer (`crates/nini-core/src/trust_gate.rs`, listed above), so
    // that it happens **after** the store's lock is released — a blocked
    // print under that lock stalled every other tool call. The store's
    // own `check` is now test-only, so the file has no stderr write at
    // all and a whitelist entry for it would be a stale exemption.
    (
        "crates/nini-cli/src/app.rs",
        16,
        "the TUI host: startup diagnostics, the no-TTY usage text (printed \
         before the TUI starts), and the agent stream error — which the \
         guard turns into a transcript notice while the TUI runs",
    ),
    (
        "crates/nini-cli/src/main.rs",
        2,
        "the refused-flag messages: printed before the TUI starts, and \
         they are the reason the user is told",
    ),
    (
        "crates/nini-cli/src/demo.rs",
        15,
        "`nini demo` narrates its script to stderr: a one-shot command \
         that never opens a TUI",
    ),
    (
        "crates/nini-cli/src/provider_factory.rs",
        1,
        "an unknown provider is reported and the process exits(2) before \
         the TUI starts",
    ),
    (
        "crates/nini-tools/src/operations.rs",
        1,
        "the default bash backend's `kill_pg` is a documented no-op that \
         warns when it is called. It has no production caller (the \
         timeout is the real mechanism), and the guard would catch it \
         anyway",
    ),
    (
        "crates/nini-tools/src/bash.rs",
        1,
        "tool diagnostics; the guard catches them while the TUI runs",
    ),
    (
        "crates/nini-session/src/lib.rs",
        3,
        "session-file diagnostics; the guard catches them while the TUI \
         runs",
    ),
];

/// The macros that write to **stdout** (the `!` is matched separately,
/// because Rust allows whitespace between the name and the `!`).
const STDOUT_MACROS: &[&str] = &["print", "println"];

/// The macros that write to **stderr**.
const STDERR_MACROS: &[&str] = &["eprint", "eprintln"];

/// Function-call spellings of the same thing.
///
/// `writeln!(std::io::stdout(), …)` is a stdout write with a different
/// spelling, and a test that only greps for the macros misses it.
///
/// Only the *write* spellings count: a handle taken on its own
/// (`let mut out = std::io::stdout();`) is not a write, and handles are
/// taken in perfectly legitimate places — the panic hook restores the
/// terminal through one.
///
/// Known limitation: a write through a handle variable
/// (`let mut out = io::stdout(); writeln!(out, …)`) is not detected.
/// The macro list catches the common spelling, and the terminal guard
/// catches stderr anyway.
const STDOUT_CALLS: &[&str] = &[
    "writeln!(std::io::stdout(",
    "writeln!(io::stdout(",
    "write!(std::io::stdout(",
    "write!(io::stdout(",
    "print!(std::io::stdout(",
    "print!(io::stdout(",
];
const STDERR_CALLS: &[&str] = &[
    "writeln!(std::io::stderr(",
    "writeln!(io::stderr(",
    "write!(std::io::stderr(",
    "write!(io::stderr(",
    "eprint!(std::io::stderr(",
    "eprint!(io::stderr(",
];

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
                let terminator = format!("\"{}", "#".repeat(hashes));
                let rest: String = chars[j..].iter().collect();
                if let Some(end) = rest.find(&terminator) {
                    let consumed = end + terminator.len();
                    for _ in 0..consumed {
                        out.push(' ');
                    }
                    i = j + consumed;
                    continue;
                }
            }
        }
        if c == '"' {
            // Ordinary string: copy it out, blanked.
            out.push(' ');
            i += 1;
            while i < n {
                if chars[i] == '\\' {
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                    continue;
                }
                if chars[i] == '"' {
                    out.push(' ');
                    i += 1;
                    break;
                }
                if chars[i] == '\n' {
                    out.push('\n');
                } else {
                    out.push(' ');
                }
                i += 1;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Blank out `#[cfg(test)]` modules: a test's `eprintln!` is
/// diagnostics for the test runner, not a write to the user's screen.
fn strip_cfg_test(src: &str) -> String {
    let mut out = src.to_string();
    let mut from = 0usize;
    while let Some(rel) = out[from..].find("#[cfg(test)]") {
        let start = from + rel;
        let brace = match out[start..].find('{') {
            Some(b) => start + b,
            None => break,
        };
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
        // The replacement is one byte per character, so the string may
        // have *shrunk*: `end` was an index into the old string and is
        // past the end of the new one. Continue from where the
        // replacement ends, in the new coordinates.
        from = start + blanked.len();
    }
    out
}

/// The line numbers of the given spellings in one source file.
fn sites_of(src: &str, macros: &[&str], calls: &[&str]) -> Vec<usize> {
    let code = strip_cfg_test(&code_only(src));
    let mut sites = Vec::new();
    for needle in macros.iter().chain(calls.iter()) {
        let mut from = 0usize;
        while let Some(pos) = code[from..].find(needle) {
            let at = from + pos;
            from = at + needle.len();
            // Not a call if it is part of a longer identifier.
            let before = code[..at].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            // A macro call is `name` `!` `(` — and Rust allows
            // whitespace between the name and the `!`, so `print ! ("…")`
            // is a call to stdout. The name is matched without the `!`
            // for exactly that reason.
            let mut probe = from;
            while probe < code.len() && code[probe..].starts_with(char::is_whitespace) {
                probe += code[probe..]
                    .chars()
                    .next()
                    .map(|c| c.len_utf8())
                    .unwrap_or(1);
            }
            if !code[probe..].starts_with('!') {
                continue;
            }
            probe += 1;
            while probe < code.len() && code[probe..].starts_with(char::is_whitespace) {
                probe += code[probe..]
                    .chars()
                    .next()
                    .map(|c| c.len_utf8())
                    .unwrap_or(1);
            }
            if !code[probe..].starts_with('(') {
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

/// The crates whose sources are checked: every crate in the workspace.
fn crate_dirs() -> Vec<String> {
    let root = repo_root();
    let mut dirs: Vec<String> = Vec::new();
    let Ok(entries) = std::fs::read_dir(root.join("crates")) else {
        return dirs;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.join("src").is_dir() {
            dirs.push(format!(
                "crates/{}/src",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
    }
    dirs.sort();
    dirs
}

/// Check one stream of one file against its whitelist.
fn check_stream(
    rel: &str,
    kind: &str,
    sites: &[usize],
    whitelist: &[(&str, usize, &str)],
    problems: &mut Vec<String>,
) {
    let entry = whitelist.iter().find(|(w, _, _)| *w == rel);
    match entry {
        Some((_, expected, reason)) => {
            if sites.len() != *expected {
                problems.push(format!(
                    "{rel}: {} {kind} write(s) (lines {}), the whitelist says \
                     {expected}. A new write here needs a decision, not a bigger number — {reason}",
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
            "{rel}: writes to {kind} on line(s) {}. {kind} is a screen the TUI is painting: \
             use the notice channel (`AppState::push_notice`), or add a whitelist entry with a \
             reason.",
            sites
                .iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[test]
fn no_source_file_writes_to_the_terminal_outside_the_whitelist() {
    let root = repo_root();
    let mut problems: Vec<String> = Vec::new();

    for crate_dir in crate_dirs() {
        let dir = root.join(&crate_dir);
        assert!(dir.is_dir(), "missing source directory: {}", dir.display());
        for path in rust_files(&dir) {
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let src = std::fs::read_to_string(&path).expect("read source");
            let stdout_sites = sites_of(&src, STDOUT_MACROS, STDOUT_CALLS);
            let stderr_sites = sites_of(&src, STDERR_MACROS, STDERR_CALLS);
            if stdout_sites.is_empty() && stderr_sites.is_empty() {
                continue;
            }
            if !stdout_sites.is_empty() {
                check_stream(
                    &rel,
                    "stdout",
                    &stdout_sites,
                    STDOUT_WHITELIST,
                    &mut problems,
                );
            }
            if !stderr_sites.is_empty() {
                check_stream(
                    &rel,
                    "stderr",
                    &stderr_sites,
                    STDERR_WHITELIST,
                    &mut problems,
                );
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
    for (rel, _, _) in STDOUT_WHITELIST.iter().chain(STDERR_WHITELIST.iter()) {
        assert!(
            root.join(rel).is_file(),
            "the whitelist names {rel}, which does not exist"
        );
    }
}

#[test]
fn no_whitelist_entry_has_gone_stale() {
    // A whitelisted file that no longer writes at all is an exemption
    // for a problem that no longer exists — it hides the *next* write
    // in that file behind a number nobody checks.
    let root = repo_root();
    let mut problems: Vec<String> = Vec::new();
    for (rel, expected, reason) in STDOUT_WHITELIST {
        let src = std::fs::read_to_string(root.join(rel)).expect("read source");
        let sites = sites_of(&src, STDOUT_MACROS, STDOUT_CALLS);
        if sites.len() != *expected {
            problems.push(format!(
                "{rel}: the whitelist says {expected} stdout write(s), the file has {}. \
                 A stale exemption is a silent one — {reason}",
                sites.len()
            ));
        }
    }
    for (rel, expected, reason) in STDERR_WHITELIST {
        let src = std::fs::read_to_string(root.join(rel)).expect("read source");
        let sites = sites_of(&src, STDERR_MACROS, STDERR_CALLS);
        if sites.len() != *expected {
            problems.push(format!(
                "{rel}: the whitelist says {expected} stderr write(s), the file has {}. \
                 A stale exemption is a silent one — {reason}",
                sites.len()
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "stale whitelist entries:\n{}",
        problems.join("\n")
    );
}

#[test]
fn the_crates_that_can_write_to_the_terminal_are_all_checked() {
    // A crate that is not scanned is a crate whose writes nobody
    // looks at. This test fails when a crate is added and not put
    // under the scan, so the coverage cannot quietly shrink.
    let root = repo_root();
    let mut crates: Vec<String> = Vec::new();
    let Ok(entries) = std::fs::read_dir(root.join("crates")) else {
        panic!("no crates directory");
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.join("src").is_dir() {
            crates.push(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
            );
        }
    }
    crates.sort();
    // The expected list is written out, not derived from the same
    // function the scan uses: a test that compares a thing with itself
    // cannot fail, and a test that cannot fail is a false assurance.
    // Adding a crate to the workspace means adding it here, and the
    // scan has to cover it.
    const EXPECTED: &[&str] = &[
        "nini-ai",
        "nini-cli",
        "nini-conformance",
        "nini-core",
        "nini-ext",
        "nini-session",
        "nini-tools",
        "nini-tui",
    ];
    let scanned = crate_dirs()
        .iter()
        .map(|d| {
            d.trim_start_matches("crates/")
                .trim_end_matches("/src")
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        scanned,
        EXPECTED.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        "the crates whose sources are scanned for terminal writes changed — \
         a new crate has to be scanned, and a removed one has to go"
    );
    assert_eq!(
        crates,
        EXPECTED.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        "the workspace has a crate the scan does not know about"
    );
}
