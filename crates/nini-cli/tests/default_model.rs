//! Oracle: nini's default model is the one the operator's setup declares.
//!
//! # The change
//!
//! The default model was `MiniMax-M3.1-Flash-Preview`; it is now
//! `MiniMax-M2.7-highspeed` — the model the operator's `models.json`
//! declares under the gateway provider that serves it.
//!
//! # Why this needs an oracle
//!
//! A default model is a promise made to a user who types nothing: it is
//! the model that will answer. Nothing in the build checks it, and a
//! default that names a model the server has never heard of produces a
//! 404 at the first turn — the exact failure the previous default
//! (`test-model`) was replaced for.
//!
//! So the promise is pinned in two places that cannot drift apart
//! silently: the constant in the source, and the `--help` text the
//! binary actually prints.

use std::process::Command;

/// The model nini talks to when the user names none.
const EXPECTED_DEFAULT: &str = "MiniMax-M2.7-highspeed";

#[test]
fn the_source_constant_is_the_default() {
    let main_rs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    let src = std::fs::read_to_string(&main_rs)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", main_rs.display()));
    let needle = format!("pub const DEFAULT_MODEL: &str = \"{EXPECTED_DEFAULT}\";");
    assert!(
        src.contains(&needle),
        "the default model constant is not {EXPECTED_DEFAULT:?} — the source \
         says something else, so the binary would ask for a model nobody \
         declared."
    );
}

#[test]
fn the_help_text_advertises_the_same_default() {
    let out = Command::new(env!("CARGO_BIN_EXE_nini"))
        .arg("--help")
        .output()
        .expect("nini --help runs");
    let text = String::from_utf8_lossy(&out.stdout);
    // clap prints the default on the line after the flag's own paragraph.
    let model_idx = text
        .lines()
        .position(|l| l.contains("--model"))
        .unwrap_or_else(|| panic!("no --model in --help:\n{text}"));
    let paragraph: String = text
        .lines()
        .skip(model_idx)
        .take(8)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        paragraph.contains(EXPECTED_DEFAULT),
        "the help text does not name the default model:\n{paragraph}"
    );
}

#[test]
fn the_model_the_user_declares_is_listed() {
    // `--list-models` is the command a user runs to find out which
    // models they can pick. The default model reaches this installation
    // through the user's own `models.json`, so the list has to contain
    // it — the shipped catalog alone does not know it.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("models.json"),
        r#"{"providers":{"gw":{"api":"openai-completions","baseUrl":"http://127.0.0.1:9877/v1",
        "models":[{"id":"MiniMax-M2.7-highspeed","reasoning":true,"contextWindow":204800,"maxTokens":49152}]}}}"#,
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_nini"))
        .env("NINI_AGENT_DIR", dir.path())
        .args(["--list-models", "*highspeed*"])
        .output()
        .expect("nini --list-models runs");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(EXPECTED_DEFAULT),
        "the model the user declared is not in the list: {text:?}"
    );
}
