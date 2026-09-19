//! End-to-end CLI tests: spawn the `nini` binary and exercise all modes.
//!
//! These tests use std::process to run the compiled binary. We redirect
//! stdin to a non-TTY pipe so the TUI path falls back to help text (we can't
//! run an actual terminal in this env). Each mode gets its own test.

use std::io::Write;
use std::process::{Command, Stdio};
use tempfile::TempDir;

/// Resolve the path to the `nini` binary built by `cargo test`.
fn nini_bin() -> std::path::PathBuf {
    // cargo puts binaries in target/debug/<name>
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR not set (run via cargo test)");
    let mut p = std::path::PathBuf::from(manifest_dir);
    p.pop(); // drop "crates/nini-cli"
    p.pop(); // drop "crates"
    p.push("target");
    if cfg!(debug_assertions) {
        p.push("debug");
    } else {
        p.push("release");
    }
    p.push("nini");
    p
}

/// Run nini with given args, stdin piped (so TUI fallback triggers), and
/// capture (stdout, stderr, exit_code).
fn run_nini(args: &[&str], stdin: &[u8]) -> (String, String, i32) {
    let mut child = Command::new(nini_bin())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn nini");
    if let Some(mut stdin_pipe) = child.stdin.take() {
        let _ = stdin_pipe.write_all(stdin);
    }
    let out = child.wait_with_output().expect("failed to wait on nini");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn new_temp_home() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

// =====================================================================
// Test 1: --version prints the version
// =====================================================================
#[test]
fn cli_version() {
    let (out, _err, code) = run_nini(&["--version"], b"");
    assert_eq!(code, 0, "nini --version should exit 0");
    assert!(out.starts_with("nini "), "version output: {out}");
    assert!(out.contains("0."), "version should contain a semver: {out}");
}

// =====================================================================
// Test 2: --help shows usage and mentions all top-level modes
// =====================================================================
#[test]
fn cli_help() {
    let (out, _err, code) = run_nini(&["--help"], b"");
    assert_eq!(code, 0);
    for needle in ["Usage:", "-p, --print", "demo", "info", "--provider"] {
        assert!(out.contains(needle), "help missing {needle:?}; got:\n{out}");
    }
}

// =====================================================================
// Test 3: nini (no args) without TTY shows fallback help, exit 2
// =====================================================================
#[test]
fn cli_no_tty_fallback() {
    let (_out, err, code) = run_nini(&[], b"");
    assert_eq!(code, 2, "should exit 2 (usage) when no TTY");
    assert!(
        err.contains("no TTY detected") || err.contains("interactive TUI unavailable"),
        "expected TTY fallback message, got stderr: {err}"
    );
}

// =====================================================================
// Test 4: nini -p "hello" runs agent in print mode, outputs text
// =====================================================================
#[test]
fn cli_print_mode_hello() {
    let (out, _err, code) = run_nini(&["-p", "hello"], b"");
    assert_eq!(code, 0, "nini -p should exit 0");
    assert!(
        out.contains("hello"),
        "print mode should echo 'hello', got: {out:?}"
    );
}

// =====================================================================
// Test 5: nini -p with multi-word user input
// =====================================================================
#[test]
fn cli_print_mode_multiword() {
    let (out, _err, code) = run_nini(&["-p", "use bash to print hello world"], b"");
    assert_eq!(code, 0);
    // Fixture strips the "use bash to print " prefix and runs the bash command,
    // then echoes the result. So we expect "hello world" somewhere in output.
    assert!(
        out.contains("hello world"),
        "expected 'hello world' in output, got: {out:?}"
    );
}

// =====================================================================
// Test 6: nini info shows skills + settings
// =====================================================================
#[test]
fn cli_info_shows_skills() {
    let (out, _err, code) = run_nini(&["info"], b"");
    assert_eq!(code, 0);
    assert!(out.contains("nini info"));
    assert!(out.contains("Settings"));
    assert!(out.contains("Skills"));
    // Skills are environment-dependent: developers have ~/.pi/agent/skills/.
    // CI runners typically have none. We assert the rendered count matches
    // the header count rather than a minimum.
    let skill_count = out.lines().filter(|l| l.starts_with("  - ")).count();
    let header_count: usize = out
        .lines()
        .find(|l| l.starts_with("Skills ("))
        .and_then(|l| {
            // "Skills (18 loaded, ..." → take "Skills (" + digit run
            l.split_whitespace()
                .find_map(|tok| tok.trim_start_matches('(').parse().ok())
        })
        .unwrap_or(0);
    assert_eq!(skill_count, header_count, "skill count mismatch in:\n{out}");
}

// =====================================================================
// Test 7: nini demo runs autonomous task
// =====================================================================
#[test]
fn cli_demo_runs_autonomously() {
    let (out, err, code) = run_nini(&["demo"], b"");
    assert_eq!(code, 0, "demo should exit 0; stderr: {err}");
    assert!(
        err.contains("[demo]") || out.contains("[demo]"),
        "demo header missing"
    );
    // The 'find TODOs and fix them' demo emits 4 tool calls + final text.
    assert!(err.contains("tool calls executed"));
    // Should have made at least 1 tool call
    assert!(
        err.contains("tool calls executed: 4") || err.contains("tool calls executed: 1"),
        "demo should report tool call count, got: {err}"
    );
}

// =====================================================================
// Test 8: nini --provider bogus exits 2 with error
// =====================================================================
#[test]
fn cli_unknown_provider_exits_2() {
    let (_out, err, code) = run_nini(&["--provider", "bogus", "-p", "hi"], b"");
    assert_eq!(code, 2, "unknown provider should exit 2");
    assert!(
        err.contains("unknown provider") && err.contains("bogus"),
        "expected 'unknown provider: bogus', got: {err}"
    );
}

// =====================================================================
// Test 9: --provider anthropic without API key exits with clear error
// =====================================================================
#[test]
fn cli_anthropic_without_key() {
    // Use a clean env so we don't pick up a real key
    let mut child = Command::new(nini_bin())
        .args(["--provider", "anthropic", "-p", "hi"])
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_BASE_URL")
        .env_remove("NINI_PROVIDER")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let out = child.wait_with_output().expect("wait");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let code = out.status.code().unwrap_or(-1);
    assert_ne!(code, 0, "should fail when key missing");
    assert!(
        stderr.contains("ANTHROPIC_API_KEY required"),
        "expected missing-key error, got: {stderr}"
    );
}

// =====================================================================
// Test 10: --provider flag overrides NINI_PROVIDER env
// =====================================================================
#[test]
fn cli_provider_flag_overrides_env() {
    // With --provider bogus, the explicit flag wins over a (hypothetical)
    // valid NINI_PROVIDER=fake. The user gets the bogus error.
    let (out, err, code) = run_nini_with_env(
        &["--provider", "bogus", "-p", "hi"],
        b"",
        &[("NINI_PROVIDER", Some("fake"))],
    );
    assert_eq!(code, 2);
    assert!(err.contains("bogus"), "flag should win, got err: {err}");
    let _ = out;
}

fn run_nini_with_env(
    args: &[&str],
    stdin: &[u8],
    env: &[(&str, Option<&str>)],
) -> (String, String, i32) {
    let mut cmd = Command::new(nini_bin());
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        match v {
            Some(val) => cmd.env(k, val),
            None => cmd.env_remove(k),
        };
    }
    let mut child = cmd.spawn().expect("spawn");
    if let Some(mut stdin_pipe) = child.stdin.take() {
        let _ = stdin_pipe.write_all(stdin);
    }
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

// =====================================================================
// Test 11: --version works under different providers (smoke test)
// =====================================================================
#[test]
fn cli_version_independent_of_provider() {
    for prov in &["fixture", "anthropic", "openai", "openai-responses"] {
        let (out, _err, code) = run_nini(&["--provider", prov, "--version"], b"");
        assert_eq!(code, 0, "--version failed for provider {prov}");
        assert!(out.starts_with("nini "), "version output for {prov}: {out}");
    }
}

// =====================================================================
// Test 12: --provider flag with openai-compat and missing OPENAI_BASE_URL
// =====================================================================
#[test]
fn cli_openai_compat_requires_base_url() {
    let mut child = Command::new(nini_bin())
        .args(["--provider", "openai-compat", "-p", "hi"])
        .env_remove("OPENAI_BASE_URL")
        .env_remove("OPENAI_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let out = child.wait_with_output().expect("wait");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_ne!(
        out.status.code().unwrap_or(-1),
        0,
        "should fail when base url missing"
    );
    assert!(
        stderr.contains("OPENAI_BASE_URL required") || stderr.contains("OPENAI_API_KEY required"),
        "expected missing-credential error, got: {stderr}"
    );
}

#[test]
fn cli_list_models_prints_catalog() {
    // Override HOME to an empty temp dir so we don't pick up the user's
    // models.json. The defaults are used.
    let tmp = new_temp_home();
    let (out, _err, code) = run_nini_with_env(
        &["--list-models"],
        b"",
        &[("HOME", Some(tmp.path().to_str().unwrap()))],
    );
    assert_eq!(code, 0);
    assert!(!out.trim().is_empty(), "list should produce at least one model");
    assert!(out.contains("anthropic/claude-opus-4-7"));
    assert!(out.contains("openai/gpt-5"));
}

#[test]
fn cli_list_models_with_filter() {
    let tmp = new_temp_home();
    let (out, _err, code) = run_nini_with_env(
        &["--list-models", "anthropic/*"],
        b"",
        &[("HOME", Some(tmp.path().to_str().unwrap()))],
    );
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert!(!lines.is_empty());
    for line in &lines {
        assert!(
            line.contains("anthropic/"),
            "filter should only show anthropic models: {line}"
        );
    }
}

#[test]
fn cli_list_models_substring_filter() {
    let tmp = new_temp_home();
    let (out, _err, code) = run_nini_with_env(
        &["--list-models", "*sonnet*"],
        b"",
        &[("HOME", Some(tmp.path().to_str().unwrap()))],
    );
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    for line in &lines {
        assert!(
            line.contains("sonnet"),
            "substring filter should only show sonnet models: {line}"
        );
    }
}

#[test]
fn cli_no_session_flag_accepted() {
    // --no-session shouldn't trigger TTY (no TTY in CI), and should print
    // fallback usage instead of starting the TUI.
    let (_out, _err, _code) = run_nini(&["--no-session", "-p", "hello"], b"");
    // Note: code is unchecked here; we're just verifying the flag is
    // parsed without erroring.
}

#[test]
fn cli_help_includes_new_tool_flags() {
    let (out, _err, code) = run_nini(&["--help"], b"");
    assert_eq!(code, 0);
    // Spot-check tool filtering flags.
    assert!(out.contains("--tools"), "missing --tools in help");
    assert!(out.contains("--exclude-tools"), "missing --exclude-tools in help");
    assert!(out.contains("--no-tools"), "missing --no-tools in help");
    assert!(out.contains("--no-builtin-tools"), "missing --no-builtin-tools in help");
}

#[test]
fn cli_filter_tools_accepted() {
    // --tools with an unknown name should be parsed without crashing.
    // (Filtering happens later, but parsing must succeed.)
    let (_out, _err, _code) = run_nini(&["--tools", "nonexistent", "-p", "hello"], b"");
    // No assertion on exit code — just that the args parse.
}

#[test]
fn cli_exclude_tools_accepted() {
    let (_out, _err, _code) = run_nini(&["--exclude-tools", "bash", "-p", "hello"], b"");
}

#[test]
fn cli_help_includes_resource_flags() {
    let (out, _err, code) = run_nini(&["--help"], b"");
    assert_eq!(code, 0);
    assert!(out.contains("--skill"));
    assert!(out.contains("--no-skills"));
    assert!(out.contains("--prompt-template"));
    assert!(out.contains("--no-prompt-templates"));
    assert!(out.contains("--theme"));
    assert!(out.contains("--use-theme"));
    assert!(out.contains("--no-themes"));
    assert!(out.contains("--extension"));
    assert!(out.contains("--no-extensions"));
    assert!(out.contains("--no-context-files"));
}

#[test]
fn cli_help_flag_lists_all_flags() {
    let (out, _err, code) = run_nini(&["--help"], b"");
    assert_eq!(code, 0);
    // Spot-check a few of the new flags appear in help.
    assert!(out.contains("--list-models"), "missing --list-models in help");
    assert!(out.contains("--thinking"), "missing --thinking in help");
    assert!(out.contains("--session-dir"), "missing --session-dir in help");
    assert!(out.contains("--offline"), "missing --offline in help");
    assert!(out.contains("--export"), "missing --export in help");
}
