//! D1, the last step: the three symbols that were **in place but not
//! wired** are now in the runtime.
//!
//! `crates/nini-tui/tests/d1_terminal_diagnostics.rs` covers the
//! *bricks*: `terminal_title`, `tmux_setup::warning_message`,
//! `format_diagnostics`. Bricks in a box are not a house, and the test
//! that proves the difference is a test that reaches the runtime — which
//! lives behind a TTY and cannot be called from a test.
//!
//! So this file does what the other one could not do:
//!
//! * it calls the **runtime's own seam functions** —
//!   [`nini_tui::runtime::update_startup_title`] and
//!   [`nini_tui::runtime::push_startup_diagnostics`] — and asserts what
//!   the user would see;
//! * it asserts, **at the source level**, that `run()` really calls
//!   them (`include_str!` + a string search, the way
//!   `tests/hyperlink_limitation_documented.rs` does it). Without this,
//!   a `run()` that never called the two functions would still pass every
//!   other test in this file: the symbols would be alive, but dead.
//!
//! Every expected value is written out in full. The two tmux warnings are
//! Pi's own wording, quoted from the 0.85.1 snapshot
//! (`references/pi-0.85.1/src/ui/interactive-mode.ts:1198` and `:1203`).

use std::path::PathBuf;

use nini_core::skills::{load_skills_from_roots, SkillOrigin, SkillRoot, SkillSource};
use nini_tui::runtime::{push_startup_diagnostics, shared_state, update_startup_title};
use nini_tui::state::AppState;
use nini_tui::tmux_setup::{
    format_diagnostics, ResourceDiagnostic, ResourceKind, TmuxKeyboardStatus,
};

/// `runtime.rs`, read so that the *wiring* can be checked: the real
/// `run()` is behind a TTY, and a symbol nobody calls is a symbol that
/// does not exist.
const RUNTIME_SRC: &str = include_str!("../src/runtime.rs");
/// `app.rs` — the CLI side of the diagnostics wiring.
const APP_SRC: &str = include_str!("../../nini-cli/src/app.rs");
/// `skills.rs` — where the collision is produced.
const SKILLS_SRC: &str = include_str!("../../nini-core/src/skills.rs");

/// Pi's first warning, verbatim
/// (`references/pi-0.85.1/src/ui/interactive-mode.ts:1198`).
const PI_WARNING_EXTENDED_KEYS_OFF: &str =
    "tmux extended-keys is off. Modified Enter keys may not work. \
     Add `set -g extended-keys on` to ~/.tmux.conf and restart tmux.";

/// Pi's second warning, verbatim, with "Pi" replaced by "nini"
/// (`references/pi-0.85.1/src/ui/interactive-mode.ts:1203`).
const PI_WARNING_XTERM_FORMAT: &str =
    "tmux extended-keys-format is xterm. nini works best with csi-u. \
     Add `set -g extended-keys-format csi-u` to ~/.tmux.conf and restart tmux.";

/// The transcript text of the shared state, joined.
fn transcript_text(shared: &nini_tui::runtime::SharedState) -> String {
    let g = nini_tui::runtime::lock_state(shared);
    g.transcript_state
        .lines
        .iter()
        .map(|l| format!("{l:?}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A state with a session id and a cwd, the way the CLI's bootstrap
/// leaves it.
fn state_with(session_id: Option<&str>, cwd: &str) -> nini_tui::runtime::SharedState {
    let mut state = AppState::default();
    state.session_state.session_id = session_id.map(|s| s.to_string());
    state.session_state.cwd = Some(PathBuf::from(cwd));
    shared_state(state)
}

// ---------------------------------------------------------------------------
// 1. The title, with a session id: Pi's three-segment shape
// ---------------------------------------------------------------------------

#[test]
fn the_startup_title_has_the_three_segment_shape_when_there_is_a_session() {
    let shared = state_with(
        Some("u_2f1c8b40-0000-7000-8000-000000000001"),
        "/home/jin/nini",
    );
    let mut seen: Vec<String> = Vec::new();
    update_startup_title(&shared, |t| seen.push(t.to_string()));

    assert_eq!(
        seen,
        vec!["nini - u_2f1c8b40-0000-7000-8000-000000000001 - nini".to_string()],
        "the title must be `nini - <session id> - <cwd basename>`"
    );
    assert_eq!(seen[0].split(" - ").count(), 3);
    assert!(seen[0].starts_with("nini - "));
    assert!(
        seen[0].ends_with(" - nini"),
        "the last segment is the cwd basename"
    );
}

// ---------------------------------------------------------------------------
// 2. The title, without a session id: Pi's degraded two-segment shape
// ---------------------------------------------------------------------------

#[test]
fn the_startup_title_degrades_to_two_segments_without_a_session() {
    // The TUI starts before any session file exists, so the id is
    // absent — Pi's second form
    // (`interactive-mode.ts:1026`: `${APP_TITLE} - ${cwdBasename}`).
    let shared = state_with(None, "/home/jin/nini");
    let mut seen: Vec<String> = Vec::new();
    update_startup_title(&shared, |t| seen.push(t.to_string()));

    assert_eq!(seen, vec!["nini - nini".to_string()]);
    assert_eq!(seen[0].split(" - ").count(), 2);
    // No invented session id: nothing that looks like one may appear.
    assert!(
        !seen[0].contains("u_"),
        "a missing session must not turn into a made-up one: {}",
        seen[0]
    );
    // A blank id is a missing id, not a third empty segment.
    let shared = state_with(Some("   "), "/home/jin/nini");
    let mut seen: Vec<String> = Vec::new();
    update_startup_title(&shared, |t| seen.push(t.to_string()));
    assert_eq!(seen, vec!["nini - nini".to_string()]);
}

// ---------------------------------------------------------------------------
// 3. The title goes through the injected writer — no terminal write
// ---------------------------------------------------------------------------

#[test]
fn the_startup_title_is_written_through_the_injected_writer_only() {
    let shared = state_with(Some("u_abc"), "/tmp/work");
    let mut seen: Vec<String> = Vec::new();
    update_startup_title(&shared, |t| seen.push(t.to_string()));

    // Exactly one title, and it is plain text: the real sink (crossterm's
    // `SetTitle`) wraps it in an OSC 2 sequence at the call site in
    // `run()`; what the writer receives is the text alone.
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0], "nini - u_abc - work");
    assert!(
        !seen[0].contains('\u{1b}') && !seen[0].contains('\u{7}'),
        "the title text must carry no control sequence: {:?}",
        seen[0]
    );

    // The runtime's own call site goes through the real sink, and the
    // sink is crossterm's `SetTitle` — a control sequence, not a `println!`.
    assert!(
        RUNTIME_SRC.contains("crate::terminal_title::write_title_to_terminal(title)"),
        "the runtime must write the title through the OSC 2 sink"
    );
    assert!(
        !RUNTIME_SRC.contains("println!(\"nini - "),
        "the title must never be printed as text"
    );
}

// ---------------------------------------------------------------------------
// 4./5. The tmux warning goes into the transcript, in Pi's words
// ---------------------------------------------------------------------------

#[test]
fn the_extended_keys_warning_lands_in_the_transcript() {
    let shared = state_with(None, "/home/jin/nini");
    push_startup_diagnostics(&shared, TmuxKeyboardStatus::ExtendedKeysOff);

    let transcript = transcript_text(&shared);
    assert!(
        transcript.contains(PI_WARNING_EXTENDED_KEYS_OFF),
        "the transcript must carry Pi's first warning verbatim; got: {transcript}"
    );
}

#[test]
fn the_xterm_format_warning_lands_in_the_transcript() {
    let shared = state_with(None, "/home/jin/nini");
    push_startup_diagnostics(&shared, TmuxKeyboardStatus::XtermFormat);

    let transcript = transcript_text(&shared);
    assert!(
        transcript.contains(PI_WARNING_XTERM_FORMAT),
        "the transcript must carry Pi's second warning verbatim; got: {transcript}"
    );
    // The two warnings are different sentences, not one of them twice.
    assert!(!transcript.contains(PI_WARNING_EXTENDED_KEYS_OFF));
}

// ---------------------------------------------------------------------------
// 6. A setup that is fine — or that could not be read — says nothing
// ---------------------------------------------------------------------------

#[test]
fn a_fine_or_unreadable_tmux_setup_produces_no_warning() {
    // Pi: "If we couldn't query tmux (timeout, sandbox, etc.), don't warn"
    // (`interactive-mode.ts:1195`). A check that cannot answer must not
    // cry wolf.
    for status in [
        TmuxKeyboardStatus::NotApplicable,
        TmuxKeyboardStatus::Ok,
        TmuxKeyboardStatus::Unknown,
    ] {
        let shared = state_with(None, "/home/jin/nini");
        push_startup_diagnostics(&shared, status);
        let transcript = transcript_text(&shared);
        assert_eq!(
            transcript, "",
            "{status:?} must not produce a warning; got: {transcript}"
        );
        assert!(!transcript.contains("tmux"));
    }
}

// ---------------------------------------------------------------------------
// 7. The skills collision: winner, loser, and the formatted block
// ---------------------------------------------------------------------------

#[test]
fn a_skill_name_collision_reports_its_winner_and_its_loser() {
    let tmp = tempfile::tempdir().unwrap();
    let low = tmp.path().join("low");
    let high = tmp.path().join("high");
    for dir in [&low, &high] {
        let skill_dir = dir.join("review");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: review\ndescription: a review skill\n---\nbody\n",
        )
        .unwrap();
    }

    // The roots are ordered lowest priority first, and the loader's rule
    // is "the later root wins" (`all` is reversed, then `or_insert`).
    let roots = vec![
        SkillRoot {
            source: SkillSource::User,
            origin: SkillOrigin::Claude,
            dir: low.clone(),
        },
        SkillRoot {
            source: SkillSource::User,
            origin: SkillOrigin::Nini,
            dir: high.clone(),
        },
    ];
    let loaded = load_skills_from_roots(&roots);

    // One skill was loaded; the other one was skipped — and the skip is
    // now **reported**, not silent.
    assert_eq!(loaded.skills.len(), 1);
    assert_eq!(loaded.skills[0].name, "review");
    assert_eq!(loaded.collisions.len(), 1);
    let collision = &loaded.collisions[0];
    assert_eq!(collision.name, "review");
    assert_eq!(
        collision.winner,
        high.join("review").join("SKILL.md"),
        "the winner is the one from the later root"
    );
    assert_eq!(
        collision.loser,
        low.join("review").join("SKILL.md"),
        "the loser is the one from the earlier root"
    );

    // And the CLI's mapping turns it into Pi's block: the winner with a
    // ✓, the loser with a ✗ and `(skipped)`.
    let diagnostics = vec![ResourceDiagnostic::Collision {
        kind: ResourceKind::Skill,
        name: collision.name.clone(),
        winner: collision.winner.clone(),
        loser: collision.loser.clone(),
    }];
    let text = format_diagnostics(&diagnostics);
    assert_eq!(
        text,
        format!(
            "  \"review\" collision (skill):\n    \u{2713} {}\n    \u{2717} {} (skipped)",
            high.join("review").join("SKILL.md").display(),
            low.join("review").join("SKILL.md").display(),
        )
    );
    assert!(text.contains('\u{2713}'), "the winner needs a ✓: {text}");
    assert!(
        text.contains("(skipped)"),
        "the loser needs (skipped): {text}"
    );
}

// ---------------------------------------------------------------------------
// 8. The wiring itself: `run()` really calls the two seams
// ---------------------------------------------------------------------------

#[test]
fn the_runtime_really_calls_the_startup_seams() {
    // The call sites, not the definitions.
    assert!(
        RUNTIME_SRC.contains("update_startup_title(&shared"),
        "run() must call update_startup_title"
    );
    assert!(
        RUNTIME_SRC.contains("push_startup_diagnostics(&shared"),
        "run() must call push_startup_diagnostics"
    );
    assert!(
        RUNTIME_SRC.contains("check_tmux_keyboard_setup_env()"),
        "run() must run the tmux check itself"
    );

    // The order, in the source, is the order on the screen:
    //   tmux check → alternate screen → title.
    let tmux_at = RUNTIME_SRC
        .find("check_tmux_keyboard_setup_env()")
        .expect("the tmux check is called");
    let alt_at = RUNTIME_SRC
        .find("execute!(stdout, EnterAlternateScreen)")
        .expect("the alternate screen is entered");
    let title_at = RUNTIME_SRC
        .find("update_startup_title(&shared")
        .expect("the title is set");
    assert!(
        tmux_at < alt_at,
        "the tmux check must run before the alternate screen is entered"
    );
    assert!(
        alt_at < title_at,
        "the title must be set after the alternate screen is entered"
    );

    // The warning must not go to the terminal: the transcript is the
    // only user-visible channel while the TUI runs.
    assert!(
        RUNTIME_SRC.contains("crate::tmux_setup::warning_message(status)"),
        "the warning must come from warning_message"
    );
    assert!(
        RUNTIME_SRC.contains("push_notice(shared, msg)"),
        "the warning must go into the transcript, not to the terminal"
    );
}

#[test]
fn the_cli_really_reports_the_startup_diagnostics() {
    assert!(
        APP_SRC.contains("report_startup_diagnostics(&skills)"),
        "the CLI must report the skills diagnostics at startup"
    );
    assert!(
        APP_SRC.contains("ResourceDiagnostic::Collision"),
        "the CLI must map a skills collision to a resource diagnostic"
    );
    assert!(
        APP_SRC.contains("format_diagnostics"),
        "the CLI must format the diagnostics with nini-tui's formatter"
    );
    assert!(
        SKILLS_SRC.contains("pub collisions: Vec<SkillCollision>"),
        "the skills loader must report the losers it used to drop"
    );
}
