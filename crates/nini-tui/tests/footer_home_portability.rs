//! Oracle: the footer's `~` is the running user's home, on every machine.
//!
//! # The defect (`be783e9a`)
//!
//! The footer's cwd is rendered through a home-prefix substitution, and the
//! test that covers it did this:
//!
//! ```text
//! state.session_state.cwd = Some(PathBuf::from("/home/jin/nini"));
//! assert!(line.contains("~/nini"), "expected tilde-path, got: {line}");
//! ```
//!
//! Two literals, one of them a person's. The substitution reads `$HOME`, so
//! the assertion is true exactly when `$HOME == /home/jin` — the machine it
//! was written on — and false everywhere else. On both CI runners it was
//! false, and it had been false on every runner since v0.9.1: the defect was
//! older than the release that finally shipped the CI signal about it.
//!
//! A test that encodes a developer's home directory is not a test of the
//! product, it is a test of the machine. Worse, it is a test that **cannot
//! be written correctly**: to assert the substitution you need a home, and
//! the only home a test can name without naming a person is `$HOME`.
//!
//! # The two halves of the invariant
//!
//! 1. **The product side** (`the_footer_tilde_follows_home_not_the_machine`):
//!    the substitution must follow whatever `$HOME` the process was given.
//!    Driven with two different scratch homes, asserting `~/proj` both
//!    times and asserting that the scratch path itself never reaches the
//!    screen. A cwd outside `$HOME` must come through untouched, so a
//!    constant `~/` prefix cannot pass as an implementation.
//!
//! 2. **The test side** (`a_test_may_not_assert_a_tilde_built_from_a_hardcoded_home`):
//!    within one test, a hard-coded `/home/<user>` (or `/Users/<user>`)
//!    literal and a `"~/…"` expectation may not appear together. Either
//!    anchor the path to `$HOME` and keep the tilde expectation, or use a
//!    synthetic path and assert on it literally — but a test that does both
//!    has quietly re-encoded somebody's laptop, and it will be green on it
//!    and red on every other machine, which is the definition of a test
//!    that measures the wrong thing.
//!
//! invariant: be783e9a

use nini_tui::render::render_frame;
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// `$HOME` is process-global and cargo runs tests in a binary in parallel.
static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Row 0 of the frame — the footer's cwd/branch line.
fn cwd_row(state: &AppState) -> String {
    let backend = TestBackend::new(200, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                .collect::<String>()
                .trim_end_matches(' ')
                .to_string()
        })
        .next()
        .unwrap_or_default()
}

#[test]
fn the_footer_tilde_follows_home_not_the_machine() {
    let _guard = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let scratch = tempfile::tempdir().unwrap();

    for tag in ["alpha", "omega-one-with-a-longer-name"] {
        // Two homes that look nothing like the machine running this test,
        // and nothing like each other. If the substitution had a hard-coded
        // home in it, one of these two fails.
        let home = scratch.path().join(tag);
        std::fs::create_dir_all(&home).unwrap();
        std::env::set_var("HOME", &home);

        let mut state = AppState::new("m");
        state.session_state.cwd = Some(home.join("some-project"));
        state.session_state.git_branch = Some("main".into());

        let row = cwd_row(&state);
        assert!(
            row.contains("~/some-project"),
            "the footer's home prefix did not become `~` for HOME={home:?}: {row}"
        );
        assert!(
            !row.contains(&home.display().to_string()),
            "the raw home path leaked into the footer for HOME={home:?}: {row}"
        );
        assert!(row.contains("⎇ main"), "branch missing: {row}");

        // Control: a cwd that is *not* under $HOME is not tilde-shortened.
        // Without this, a `~` hard-coded into the renderer would pass the
        // assertions above and never shorten anything.
        let mut outside = AppState::new("m");
        outside.session_state.cwd = Some(std::path::PathBuf::from("/var/tmp/not-my-home"));
        let row = cwd_row(&outside);
        assert!(
            row.contains("/var/tmp/not-my-home"),
            "a path outside $HOME must be shown in full, not shortened: {row}"
        );
        assert!(!row.contains('~'), "unexpected tilde outside $HOME: {row}");
    }
}

// ---------------------------------------------------------------------------
// 2. The test-side guard
// ---------------------------------------------------------------------------

fn repo_root() -> std::path::PathBuf {
    // crates/nini-tui -> crates -> repo
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

/// Strip comments. A path quoted in a comment is documentation of the bug,
/// not an instance of it — `render.rs` names `/home/jin/nini` at length in
/// the doc comment explaining why it must never be used again.
fn strip_comments(text: &str) -> String {
    text.lines()
        .map(|l| {
            // Keep the code, drop `// …`. Block comments are rare in this
            // corpus and a `/*` inside a string is rarer; the only block
            // comments that matter are removed by the same pass below.
            let cut = l.find("//").unwrap_or(l.len());
            &l[..cut]
        })
        .collect::<Vec<_>>()
        .join("\n")
        .lines()
        .filter(|l| !l.trim_start().starts_with("/*") && !l.trim_start().starts_with("*"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// True when the chunk hard-codes somebody's home directory.
fn has_hardcoded_home(code: &str) -> bool {
    for marker in ["/home/", "/Users/"] {
        if let Some(idx) = code.find(marker) {
            let after = &code[idx + marker.len()..];
            let user: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
                .collect();
            if !user.is_empty() {
                return true;
            }
        }
    }
    false
}

#[test]
fn a_test_may_not_assert_a_tilde_built_from_a_hardcoded_home() {
    let root = repo_root();
    let mut violations: Vec<String> = Vec::new();

    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for dir in ["crates"] {
        let dir = root.join(dir);
        collect_rs(&dir, &mut files);
    }
    assert!(
        !files.is_empty(),
        "no Rust sources found under {} — the guard is not looking at anything",
        root.display()
    );

    for file in files {
        let raw = std::fs::read_to_string(&file).unwrap();
        let code = strip_comments(&raw);
        // One chunk per test, so an innocent test elsewhere in the file
        // cannot launder a guilty one.
        for (i, chunk) in code.split("#[test]").enumerate() {
            let named: Vec<&str> = chunk
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect();
            let body = named.join("\n");
            if has_hardcoded_home(&body) && body.contains("\"~/") {
                let first = body
                    .lines()
                    .find(|l| l.contains("\"/"))
                    .unwrap_or("<no line>")
                    .trim()
                    .to_string();
                violations.push(format!(
                    "{} (chunk {i}): {first}",
                    file.strip_prefix(&root).unwrap_or(&file).display()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "{} test(s) assert a `~/…` rendering while hard-coding a home \
         directory, so they pass on one machine and fail on every other:\n  {}",
        violations.len(),
        violations.join("\n  ")
    );
}

fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().and_then(|n| n.to_str()) == Some("target") {
                continue;
            }
            collect_rs(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
            out.push(p);
        }
    }
}
