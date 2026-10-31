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

//! invariant: 8c8a009d —— 声明本文件守住了哪几条历史不变量。
//! 该标注让 scripts/invariants.py 认出这些断言；词法重叠看不出它们，

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

/// `--theme-file` was refused last release with the reason *"custom themes
/// are not implemented"*. That was wrong in a way worth pinning: the theme
/// module has `Theme::dark/light`, `load_from_file`, `parse` and `merge`
/// (871 lines, full Pi palette). What was missing was one function's
/// dispatch. So the flag is now honoured rather than refused with a stale
/// excuse.
#[test]
fn theme_file_is_no_longer_refused() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_nini"))
        .args([
            "--theme-file",
            "/tmp/whatever.json",
            "-p",
            "hi",
            "--provider",
            "fixture",
        ])
        .output()
        .expect("nini runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("--theme-file"),
        "--theme-file is still refused with a stale reason:\n{stderr}"
    );
}

/// `--use-theme light` must not be refused either, and must reach the
/// theme resolver rather than being dropped at the CLI boundary.
#[test]
fn use_theme_is_not_refused() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_nini"))
        .args(["--use-theme", "light", "-p", "hi", "--provider", "fixture"])
        .output()
        .expect("nini runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("--use-theme"),
        "--use-theme is still refused:\n{stderr}"
    );
}

/// Coverage, not enumeration.
///
/// The other tests here list the flags they care about. That style has a
/// failure mode this file was built to end: **a flag added later and dropped
/// on the floor is not caught**, because nobody remembered to add it to a
/// list. `--export <path>` is the proof — it sat in `main.rs` as `export: _`,
/// advertised in `--help` as *"Export the session to HTML at this path and
/// exit"*, and every test in this file stayed green. Measured, not suspected:
/// `nini --export /tmp/x.html -p hello` exited 0, `/tmp/x.html` did not exist,
/// and the HTML landed in `~/.nini/agent/exports/`.
///
/// So: read the binary's own source, find every field destructured as `_`,
/// and require that each one which `--help` advertises is **named** in a
/// refusal. A new silent drop fails here without anyone editing a list.
#[test]
fn no_advertised_flag_is_dropped_silently() {
    let main_rs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    let src = std::fs::read_to_string(&main_rs)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", main_rs.display()));

    // Fields destructured as `_`: the shape "accepted, then dropped".
    let mut dropped: Vec<String> = Vec::new();
    for line in src.lines() {
        let t = line.trim();
        if let Some(field) = t.strip_suffix(": _,") {
            let name = field.trim();
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
            {
                dropped.push(name.to_string());
            }
        }
    }
    // Validate the scan's *premise*, not its result. An earlier version
    // asserted that at least one `_` field exists, on the theory that an
    // empty result means the scan broke — which is backwards: after
    // `--export` and `--tui-mode` were bound, zero dropped fields is the
    // correct outcome, and the assertion turned the fix into a failure.
    assert!(
        src.contains("} = args;"),
        "the destructure this test scans for is gone from main.rs, so a clean \
         result here would mean nothing was checked"
    );

    let advertised = advertised_flags();
    let mut silent = Vec::new();
    for field in dropped {
        let flag = format!("--{}", field.replace('_', "-"));
        if !advertised.contains(&flag) {
            continue; // internal field, never advertised
        }
        let named_in_a_refusal = src.contains(&format!("\"{flag}\""));
        if !named_in_a_refusal {
            silent.push(flag);
        }
    }

    assert!(
        silent.is_empty(),
        "advertised in `--help`, destructured as `_`, and never named in a \
         refusal — so nini accepts it, documents it, and does nothing:\n  {}\n\
         Fix: bind the field and either use it or refuse it with a real reason.",
        silent.join("\n  ")
    );
}

/// v0.14: `--max-parallel-tools N` is a real flag, not a refused one.
///
/// It caps how many of a turn's tool calls run at once. `0` (the default)
/// means "no cap", the way Pi 0.85.1 runs every tool call of a turn at
/// once. The flag must not show up in the refusal list.
#[test]
fn max_parallel_tools_is_accepted_rather_than_refused() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_nini"))
        .args([
            "--max-parallel-tools",
            "4",
            "-p",
            "hi",
            "--provider",
            "fixture",
        ])
        .output()
        .expect("nini runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "`nini --max-parallel-tools 4` did not run: exit {:?}, stderr:\n{stderr}",
        out.status.code()
    );
    assert!(
        !stderr.contains("--max-parallel-tools"),
        "--max-parallel-tools is refused rather than honoured:\n{stderr}"
    );
}

/// An illegal N is handled exactly like an illegal `--max-iterations`:
/// clap's own error, exit code 2. This asserts the *difference* between
/// the two invocations rather than clap's wording, which is free to change.
#[test]
fn an_illegal_max_parallel_tools_fails_like_an_illegal_max_iterations() {
    let run = |flag: &str| {
        std::process::Command::new(env!("CARGO_BIN_EXE_nini"))
            .args([flag, "-1", "-p", "hi", "--provider", "fixture"])
            .output()
            .expect("nini runs")
            .status
            .code()
    };
    let parallel = run("--max-parallel-tools");
    let iterations = run("--max-iterations");
    assert_ne!(
        parallel,
        Some(0),
        "`--max-parallel-tools -1` was accepted — an illegal N must fail"
    );
    assert_eq!(
        parallel, iterations,
        "`--max-parallel-tools -1` and `--max-iterations -1` must fail the same way"
    );
}

/// `0` is a legal value, and it means "no ceiling" — the same spelling
/// `--max-iterations` uses. It must be accepted, not refused.
#[test]
fn zero_is_an_accepted_max_parallel_tools_value() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_nini"))
        .args([
            "--max-parallel-tools",
            "0",
            "-p",
            "hi",
            "--provider",
            "fixture",
        ])
        .output()
        .expect("nini runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "`nini --max-parallel-tools 0` did not run: exit {:?}, stderr:\n{stderr}",
        out.status.code()
    );
    assert!(
        !stderr.contains("--max-parallel-tools"),
        "--max-parallel-tools 0 is refused rather than honoured:\n{stderr}"
    );
}

/// A ceiling of 3 is a legal value too, and it is not refused.
#[test]
fn a_small_max_parallel_tools_value_is_accepted() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_nini"))
        .args([
            "--max-parallel-tools",
            "3",
            "-p",
            "hi",
            "--provider",
            "fixture",
        ])
        .output()
        .expect("nini runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "`nini --max-parallel-tools 3` did not run: exit {:?}, stderr:\n{stderr}",
        out.status.code()
    );
    assert!(
        !stderr.contains("--max-parallel-tools"),
        "--max-parallel-tools 3 is refused rather than honoured:\n{stderr}"
    );
}

/// A value that is not a number fails exactly like a value that is not a
/// number for `--max-iterations`: clap's own error, exit code 2. The
/// assertion is the *difference* between the two invocations, not clap's
/// wording.
#[test]
fn a_non_numeric_max_parallel_tools_fails_like_a_non_numeric_max_iterations() {
    let run = |flag: &str| {
        std::process::Command::new(env!("CARGO_BIN_EXE_nini"))
            .args([flag, "abc", "-p", "hi", "--provider", "fixture"])
            .output()
            .expect("nini runs")
            .status
            .code()
    };
    let parallel = run("--max-parallel-tools");
    let iterations = run("--max-iterations");
    assert_ne!(
        parallel,
        Some(0),
        "`--max-parallel-tools abc` was accepted — a non-numeric N must fail"
    );
    assert_eq!(
        parallel, iterations,
        "`--max-parallel-tools abc` and `--max-iterations abc` must fail the same way"
    );
}

/// The two ceilings are independent: giving both must work, and neither
/// may make the other fail.
#[test]
fn the_two_ceilings_can_be_given_together() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_nini"))
        .args([
            "--max-parallel-tools",
            "3",
            "--max-iterations",
            "5",
            "-p",
            "hi",
            "--provider",
            "fixture",
        ])
        .output()
        .expect("nini runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "`nini --max-parallel-tools 3 --max-iterations 5` did not run: exit {:?}, stderr:\n{stderr}",
        out.status.code()
    );
    assert!(
        !stderr.contains("--max-parallel-tools") && !stderr.contains("--max-iterations"),
        "one of the two ceilings was refused:\n{stderr}"
    );
}

/// The specific hole `no_advertised_flag_is_dropped_silently` was written for.
#[test]
fn export_with_a_path_is_refused_rather_than_swallowed() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_nini"))
        .args([
            "--export",
            "/tmp/nini-should-not-appear.html",
            "--provider",
            "fixture",
            "--offline",
            "--no-context-files",
            "-p",
            "hi",
        ])
        .output()
        .expect("nini runs");
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        stderr.contains("--export"),
        "`--export <path>` must be named when it is refused; a silent exit 0 \
         teaches the user to distrust every other message nini prints.\nstderr:\n{stderr}"
    );
    assert!(
        !std::path::Path::new("/tmp/nini-should-not-appear.html").exists(),
        "the path was named in the refusal, yet a file appeared there anyway"
    );
}

/// v0.15: `--tool-execution <MODE>` is a real flag, not a refused one.
///
/// It picks how a turn's tool calls run: `parallel` (the default) or
/// `sequential`. The flag must not show up in the refusal list.
#[test]
fn tool_execution_is_accepted_rather_than_refused() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_nini"))
        .args([
            "--tool-execution",
            "sequential",
            "-p",
            "hi",
            "--provider",
            "fixture",
        ])
        .output()
        .expect("nini runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "`nini --tool-execution sequential` did not run: exit {:?}, stderr:\n{stderr}",
        out.status.code()
    );
    assert!(
        !stderr.contains("--tool-execution"),
        "--tool-execution is refused rather than honoured:\n{stderr}"
    );
}

/// An illegal mode fails the way an illegal `--max-iterations` fails:
/// clap's own error, exit code 2. The assertion is the *difference*
/// between the two invocations, not clap's wording.
#[test]
fn an_illegal_tool_execution_fails_like_an_illegal_max_iterations() {
    let run = |value: &str| {
        std::process::Command::new(env!("CARGO_BIN_EXE_nini"))
            .args([
                "--tool-execution",
                value,
                "-p",
                "hi",
                "--provider",
                "fixture",
            ])
            .output()
            .expect("nini runs")
            .status
            .code()
    };
    let illegal_mode = run("bogus");
    let illegal_number = std::process::Command::new(env!("CARGO_BIN_EXE_nini"))
        .args([
            "--max-iterations",
            "abc",
            "-p",
            "hi",
            "--provider",
            "fixture",
        ])
        .output()
        .expect("nini runs")
        .status
        .code();
    assert_ne!(
        illegal_mode,
        Some(0),
        "`--tool-execution bogus` was accepted — an unknown mode must fail"
    );
    assert_eq!(
        illegal_mode, illegal_number,
        "`--tool-execution bogus` and `--max-iterations abc` must fail the same way"
    );
}

/// The refusal of an unknown mode must say *which* values are legal.
///
/// "invalid value" alone leaves the user with nothing to type next time.
#[test]
fn an_illegal_tool_execution_names_the_flag_and_the_legal_values() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_nini"))
        .args([
            "--tool-execution",
            "bogus",
            "-p",
            "hi",
            "--provider",
            "fixture",
        ])
        .output()
        .expect("nini runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--tool-execution"),
        "the error does not name the flag:\n{stderr}"
    );
    assert!(
        stderr.contains("parallel") && stderr.contains("sequential"),
        "the error does not name the legal values:\n{stderr}"
    );
}

/// The help must advertise the flag, and it must not leak the
/// repository's internal file layout into the user's terminal.
#[test]
fn the_help_advertises_tool_execution_without_internal_paths() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_nini"))
        .arg("--help")
        .output()
        .expect("nini --help runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("--tool-execution"),
        "the help does not advertise --tool-execution:\n{stdout}"
    );
    for leak in ["references/", "agent-loop.ts", "settings-manager.ts"] {
        assert!(
            !stdout.contains(leak),
            "the help leaks the internal path `{leak}`:\n{stdout}"
        );
    }
}
