//! Oracle: a flag in `--help` must either work or say it does not.
//!
//! # What Pi does
//!
//! `--system-prompt` becomes the `customPrompt` and `--append-system-prompt`
//! the `appendSystemPrompt` of `buildSystemPrompt`
//! (`pi-coding-agent/src/core/system-prompt.ts:28-67`):
//!
//! ```text
//! 41  const appendSection = appendSystemPrompt ? `\n\n${appendSystemPrompt}` : "";
//! 46  if (customPrompt) {
//! 47      let prompt = customPrompt;          // the base is REPLACED
//! 49      if (appendSection) prompt += appendSection;
//! 55      … <project_context> from AGENTS.md
//! 66      … formatSkillsForPrompt(skills)
//! 68  } else { … the normal base … }
//! ```
//!
//! # What nini does
//!
//! Sixteen flags are destructured as `_` in `main.rs` — accepted, listed in
//! `--help`, described ("Disable AGENTS.md / CLAUDE.md context files"), and
//! then dropped. Six more were pushed onto an `ignored` vector with one
//! message for all of them. The comment above the destructure says it
//! plainly: *"these were all `_: _` … i.e. accepted, documented in `--help`,
//! and then dropped on the floor."*
//!
//! `--system-prompt` is the sharpest case: `settings_to_system_prompt`
//! assembles a prompt from the model, the tools and the skills, and the one
//! thing a user reaching for `--system-prompt` wants — their own instructions
//! — is the one thing they cannot contribute.
//!
//! These tests pin the half that only a running binary can show: the flags
//! that cannot work are refused, each naming itself and its own reason.
//! The half that needs the prompt assembler (custom/append system prompt)
//! lives in `prompt_setup.rs`, because nini-cli is a binary crate and its
//! modules are not importable from an integration test.

/// Parse the binary's own `--help` and collect the flags it advertises.
fn advertised_flags() -> Vec<String> {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_nini"))
        .arg("--help")
        .output()
        .expect("nini --help runs");
    let text = String::from_utf8_lossy(&out.stdout);
    let mut flags = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        for tok in line.split_whitespace() {
            let tok = tok.trim_start_matches('-');
            let tok = tok
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                .next()
                .unwrap_or("");
            if tok.len() > 1 && tok.chars().next().unwrap().is_ascii_alphabetic() {
                flags.push(format!("--{tok}"));
            }
        }
    }
    flags.sort();
    flags.dedup();
    flags
}

// ---------------------------------------------------------------------------
// 2. --skill restricts discovery instead of adding to it
// ---------------------------------------------------------------------------

/// `--help` says *"Load a skill file or directory … Overrides automatic skill
/// discovery."* "Overrides" is the operative word: a user who names one
/// directory wants exactly that, not that plus everything auto-discovered.
#[test]
fn an_explicit_skill_path_replaces_auto_discovery() {
    let dir = tempfile::tempdir().unwrap();
    // A skills root: subdirectories, one SKILL.md each — the shape both
    // `~/.nini/agent/skills/` and `~/.claude/skills/` have.
    let skills_dir = dir.path().join("myskills");
    let one = skills_dir.join("mine");
    std::fs::create_dir_all(&one).unwrap();
    std::fs::write(
        one.join("SKILL.md"),
        "---\nname: mine\ndescription: does my thing\n---\n\nbody\n",
    )
    .unwrap();

    let loaded = nini_core::skills::load_skills_from_paths(&[skills_dir.clone()]);
    let names: Vec<&str> = loaded.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["mine"],
        "--skill did not load exactly the named directory: {names:?}"
    );
    assert!(
        loaded.errors.is_empty(),
        "a valid skill directory reported errors: {:?}",
        loaded.errors
    );
}

/// `--help` says "a skill file or directory". A user who copied a single
/// skill out of `~/.claude/skills/` has a directory containing `SKILL.md`
/// and no subdirectories — the loader must treat that as one skill, not as a
/// root that happens to be empty.
#[test]
fn a_single_skill_directory_is_loaded_as_itself() {
    let dir = tempfile::tempdir().unwrap();
    let one = dir.path().join("solo");
    std::fs::create_dir_all(&one).unwrap();
    std::fs::write(
        one.join("SKILL.md"),
        "---\nname: solo\ndescription: one skill\n---\n\nbody\n",
    )
    .unwrap();

    let loaded = nini_core::skills::load_skills_from_paths(&[one]);
    let names: Vec<&str> = loaded.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["solo"], "got {names:?} ({:?})", loaded.errors);
}

/// A path that does not exist is a user mistake worth naming, not a silent
/// empty skill set that looks like "no skills installed".
#[test]
fn a_missing_skill_path_is_reported() {
    let loaded = nini_core::skills::load_skills_from_paths(&[std::path::PathBuf::from(
        "/definitely/not/here",
    )]);
    assert!(
        !loaded.errors.is_empty(),
        "a nonexistent --skill path failed silently"
    );
    assert!(
        loaded.errors[0].contains("not/here"),
        "the error does not name the path: {:?}",
        loaded.errors
    );
}

// ---------------------------------------------------------------------------
// 3. Every advertised flag is either honoured or refused
// ---------------------------------------------------------------------------

/// The blanket message ("ignoring X, Y, Z: TUI session persistence is not
/// implemented yet") is wrong for every flag except the session ones: it
/// attributes a themes problem to session persistence.
#[test]
fn a_refused_flag_names_itself_and_its_own_reason() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_nini"))
        .args([
            "--offline",
            "--no-context-files",
            "--no-prompt-templates",
            "-p",
            "hi",
            "--provider",
            "fixture",
        ])
        .output()
        .expect("nini runs");
    let stderr = String::from_utf8_lossy(&out.stderr);

    for flag in ["--offline", "--no-context-files", "--no-prompt-templates"] {
        assert!(
            stderr.contains(flag),
            "`{flag}` was accepted and dropped without ever being named. \
             stderr was:\n{stderr}"
        );
    }
    assert!(
        !stderr.contains("session persistence"),
        "unrelated flags are still being explained by a sentence about \
         session persistence:\n{stderr}"
    );
}

/// Sanity: the parser still knows the flags (this test is about the ones
/// that are *not* refused, not about typos).
#[test]
fn the_flag_set_is_not_empty() {
    let flags = advertised_flags();
    assert!(flags.len() > 10, "help parsing broke: {flags:?}");
    assert!(flags.contains(&"--system-prompt".to_string()), "{flags:?}");
}
