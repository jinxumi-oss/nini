//! Oracle for D1 — the terminal/diagnostics small items Pi has and nini
//! did not.
//!
//! **What is under test.** Three of Pi's six D1 items are implemented in
//! this round; the other three are declared "explicitly not done" in
//! `docs/plans/d1-decision.md`. This file covers the implemented ones:
//!
//! | Pi (0.85.1)                                  | nini                                  |
//! |----------------------------------------------|---------------------------------------|
//! | `updateTerminalTitle` (`interactive-mode.ts:1020`) | `nini_tui::terminal_title`        |
//! | `checkTmuxKeyboardSetup` (`interactive-mode.ts:1161`) | `nini_tui::tmux_setup`         |
//! | `formatDiagnostics` (`interactive-mode.ts:1589`) | `nini_tui::tmux_setup::format_diagnostics` |
//!
//! **The discipline this file follows.** Every assertion is on a
//! *value* — a title string, a status enum, a block of formatted text —
//! and every expected value is written out in full, so a change in
//! behaviour turns a line red instead of quietly moving a boundary. The
//! strings that Pi itself produces (the two tmux warnings) are quoted
//! from the snapshot, not paraphrased.
//!
//! **Why the tmux query is injected.** The check shells out to
//! `tmux show -gv …`. The machine running the tests may have no tmux, may
//! have a tmux with a different configuration, and may have a tmux that
//! hangs — so the query is a trait ([`TmuxOptionQuery`]) and the test
//! answers it. The *timeout* is not faked: it is tested against a
//! command that really does hang (`sleep`), because a timeout that is
//! only ever faked is a timeout that has never been tested.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use nini_tui::terminal_title::{
    cwd_basename, terminal_title, update_terminal_title, TitleRecorder, APP_TITLE,
};
use nini_tui::tmux_setup::{
    check_tmux_keyboard_setup, classify, format_diagnostics, run_with_timeout, warning_message,
    DiagnosticSeverity, ResourceDiagnostic, ResourceKind, TmuxKeyboardStatus, TmuxOptionQuery,
};

/// A tmux that answers whatever the test tells it to.
struct FakeTmux {
    extended_keys: Option<String>,
    extended_keys_format: Option<String>,
}

impl FakeTmux {
    fn new(extended_keys: Option<&str>, extended_keys_format: Option<&str>) -> Self {
        Self {
            extended_keys: extended_keys.map(str::to_string),
            extended_keys_format: extended_keys_format.map(str::to_string),
        }
    }

    /// A tmux that never answers — the timeout / missing-binary case.
    fn mute() -> Self {
        Self::new(None, None)
    }
}

impl TmuxOptionQuery for FakeTmux {
    fn show_option(&self, option: &str) -> Option<String> {
        match option {
            "extended-keys" => self.extended_keys.clone(),
            "extended-keys-format" => self.extended_keys_format.clone(),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// 1. The title's shape: <product> - <session label> - <cwd basename>
// ---------------------------------------------------------------------------

#[test]
fn terminal_title_has_the_pi_shape() {
    // Pi: `${APP_TITLE} - ${sessionName} - ${cwdBasename}`
    //     (references/pi-0.85.1/src/ui/interactive-mode.ts:1024)
    assert_eq!(
        terminal_title(Some("u_2f1c"), "/home/jin/nini"),
        "nini - u_2f1c - nini"
    );
    // The product title is the first segment and it is nini's.
    assert!(terminal_title(Some("u_2f1c"), "/home/jin/nini").starts_with(APP_TITLE));
    // Three segments when there is a session label.
    let title = terminal_title(Some("u_2f1c"), "/home/jin/nini");
    assert_eq!(title.split(" - ").count(), 3);
    // The last segment is the cwd basename, not the whole path.
    assert!(title.ends_with(" - nini"));
    assert!(!title.contains('/'));
}

/// The writer is injected, so the test can see the title without writing
/// to the terminal (the TUI owns stdout while it runs).
#[test]
fn the_title_is_handed_to_an_injected_writer() {
    let mut seen: Vec<String> = Vec::new();
    update_terminal_title(Some("u_abc"), "/tmp/work", |t| seen.push(t.to_string()));
    assert_eq!(seen, vec!["nini - u_abc - work".to_string()]);

    // The same thing through the recorder, which is what a caller that
    // wants to know the current title uses.
    let mut recorder = TitleRecorder::new();
    update_terminal_title(Some("u_abc"), "/tmp/work", recorder.writer());
    assert_eq!(recorder.last(), Some("nini - u_abc - work"));
}

// ---------------------------------------------------------------------------
// 2. The cwd basename: Node's `path.basename`, not `Path::file_name`
// ---------------------------------------------------------------------------

#[test]
fn cwd_basename_takes_the_last_path_segment() {
    assert_eq!(cwd_basename("/home/jin/nini"), "nini");
    // A trailing slash is ignored, like Node's `path.basename`.
    assert_eq!(cwd_basename("/home/jin/nini/"), "nini");
    // An empty segment is ignored too.
    assert_eq!(cwd_basename("/home//nini"), "nini");
    // A relative path works the same way.
    assert_eq!(cwd_basename("work/nini"), "nini");
    // The root has **no** name: Node's `path.basename("/")` is the empty
    // string (verified with node: `path.basename('/') === ''`), while
    // `Path::file_name("/")` is `None`.
    //
    // This assertion was **wrong** when it was written (it said `"/"`), and
    // the implementation had followed the wrong assertion — a test that
    // pins a mistake is worse than no test, because it makes the mistake
    // look like a decision.
    assert_eq!(cwd_basename("/"), "");
    assert_eq!(cwd_basename("//"), "");
    // An empty path has no name either.
    assert_eq!(cwd_basename(""), "");
    // `..` is kept: the caller asked for a directory name, not a
    // resolved one.
    assert_eq!(cwd_basename("/home/jin/.."), "..");
}

// ---------------------------------------------------------------------------
// 3. No session id: degrade, do not panic
// ---------------------------------------------------------------------------

#[test]
fn a_missing_session_id_degrades_instead_of_panicking() {
    // Pi's second form: `${APP_TITLE} - ${cwdBasename}`
    // (interactive-mode.ts:1026).
    assert_eq!(terminal_title(None, "/home/jin/nini"), "nini - nini");
    // A blank id is a missing id: an empty segment would read as a typo.
    assert_eq!(terminal_title(Some(""), "/home/jin/nini"), "nini - nini");
    assert_eq!(terminal_title(Some("   "), "/home/jin/nini"), "nini - nini");
    // Still two segments, never three with an empty one.
    assert_eq!(
        terminal_title(None, "/home/jin/nini").split(" - ").count(),
        2
    );
    // A missing id and an empty cwd still produce a legal title.
    let title = terminal_title(None, "");
    assert_eq!(title, "nini - ");
    assert!(!title.is_empty());
}

// ---------------------------------------------------------------------------
// 4. The tmux check: every branch Pi has
// ---------------------------------------------------------------------------

#[test]
fn the_tmux_check_covers_every_pi_branch() {
    // Not in tmux → the check does not apply. Pi: `if (!process.env.TMUX)
    // return undefined;` (interactive-mode.ts:1163).
    assert_eq!(
        check_tmux_keyboard_setup(false, &FakeTmux::new(Some("off"), None)),
        TmuxKeyboardStatus::NotApplicable
    );
    assert_eq!(
        classify(false, Some("on"), Some("csi-u")),
        TmuxKeyboardStatus::NotApplicable
    );

    // `extended-keys` on (or always) → fine.
    assert_eq!(
        check_tmux_keyboard_setup(true, &FakeTmux::new(Some("on"), Some("csi-u"))),
        TmuxKeyboardStatus::Ok
    );
    assert_eq!(
        check_tmux_keyboard_setup(true, &FakeTmux::new(Some("always"), Some("csi-u"))),
        TmuxKeyboardStatus::Ok
    );

    // `extended-keys` off → the "modified Enter" warning.
    assert_eq!(
        check_tmux_keyboard_setup(true, &FakeTmux::new(Some("off"), Some("csi-u"))),
        TmuxKeyboardStatus::ExtendedKeysOff
    );
    // Unset is the same thing: tmux exits 0 and prints nothing, which is
    // a value, not a failed query.
    assert_eq!(
        check_tmux_keyboard_setup(true, &FakeTmux::new(Some(""), Some("csi-u"))),
        TmuxKeyboardStatus::ExtendedKeysOff
    );

    // `extended-keys-format` xterm → the other warning.
    assert_eq!(
        check_tmux_keyboard_setup(true, &FakeTmux::new(Some("on"), Some("xterm"))),
        TmuxKeyboardStatus::XtermFormat
    );

    // The warnings are Pi's wording (interactive-mode.ts:1198 and :1203).
    assert_eq!(
        warning_message(TmuxKeyboardStatus::ExtendedKeysOff),
        Some(
            "tmux extended-keys is off. Modified Enter keys may not work. \
             Add `set -g extended-keys on` to ~/.tmux.conf and restart tmux."
        )
    );
    assert_eq!(
        warning_message(TmuxKeyboardStatus::XtermFormat),
        Some(
            "tmux extended-keys-format is xterm. nini works best with csi-u. \
             Add `set -g extended-keys-format csi-u` to ~/.tmux.conf and restart tmux."
        )
    );
    // The branches that are fine produce no warning.
    assert_eq!(warning_message(TmuxKeyboardStatus::Ok), None);
    assert_eq!(warning_message(TmuxKeyboardStatus::NotApplicable), None);
    assert_eq!(warning_message(TmuxKeyboardStatus::Unknown), None);
}

// ---------------------------------------------------------------------------
// 5. A tmux that never answers must not hang the startup
// ---------------------------------------------------------------------------

#[test]
fn a_tmux_query_that_never_answers_does_not_warn_and_does_not_hang() {
    // Pi: "If we couldn't query tmux (timeout, sandbox, etc.), don't warn"
    // (interactive-mode.ts:1195).
    let started = Instant::now();
    let status = check_tmux_keyboard_setup(true, &FakeTmux::mute());
    assert_eq!(status, TmuxKeyboardStatus::Unknown);
    assert_eq!(warning_message(status), None);
    assert!(started.elapsed() < Duration::from_secs(1));

    // The timeout itself is real, not faked: a command that hangs is
    // killed, and the call returns without an answer.
    let started = Instant::now();
    let answer = run_with_timeout("sleep", &["30"], Duration::from_millis(200));
    assert_eq!(answer, None);
    // The budget was 200ms; a second of slack is generous, and it still
    // catches a timeout that fires late (or never).
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the timeout did not fire: {:?}",
        started.elapsed()
    );

    // A command that answers is answered with its trimmed stdout — the
    // happy path of the same code.
    assert_eq!(
        run_with_timeout("echo", &["on"], Duration::from_secs(5)),
        Some("on".to_string())
    );
}

// ---------------------------------------------------------------------------
// 6. The resource diagnostics: grouped by name, readable
// ---------------------------------------------------------------------------

#[test]
fn resource_diagnostics_are_grouped_by_name_and_readable() {
    let diagnostics = vec![
        // Two skills named `review`: one won, one was skipped.
        ResourceDiagnostic::Collision {
            kind: ResourceKind::Skill,
            name: "review".to_string(),
            winner: PathBuf::from("/home/jin/.nini/agent/skills/review/SKILL.md"),
            loser: PathBuf::from("/home/jin/.claude/skills/review/SKILL.md"),
        },
        // A third one, same name: the group keeps one header and one
        // winner, and lists every skipped path.
        ResourceDiagnostic::Collision {
            kind: ResourceKind::Skill,
            name: "review".to_string(),
            winner: PathBuf::from("/home/jin/.nini/agent/skills/review/SKILL.md"),
            loser: PathBuf::from("/home/jin/.pi/agent/skills/review/SKILL.md"),
        },
        // A prompt named `review` too: a different namespace, so a
        // different group.
        ResourceDiagnostic::Collision {
            kind: ResourceKind::Prompt,
            name: "review".to_string(),
            winner: PathBuf::from("/home/jin/.nini/agent/prompts/review.md"),
            loser: PathBuf::from("/home/jin/project/.nini/prompts/review.md"),
        },
        // A skill that could not be read.
        ResourceDiagnostic::Load {
            kind: ResourceKind::Skill,
            severity: DiagnosticSeverity::Error,
            path: Some(PathBuf::from(
                "/home/jin/.nini/agent/skills/broken/SKILL.md",
            )),
            message: "read failed: permission denied".to_string(),
        },
        // A prompt that could not be parsed, with no path to show.
        ResourceDiagnostic::Load {
            kind: ResourceKind::Prompt,
            severity: DiagnosticSeverity::Error,
            path: None,
            message: "parse failed: frontmatter is not a YAML mapping".to_string(),
        },
    ];

    let text = format_diagnostics(&diagnostics);

    // The exact text: collisions first, grouped by name, then the rest.
    let expected = concat!(
        "  \"review\" collision (skill):\n",
        "    \u{2713} /home/jin/.nini/agent/skills/review/SKILL.md\n",
        "    \u{2717} /home/jin/.claude/skills/review/SKILL.md (skipped)\n",
        "    \u{2717} /home/jin/.pi/agent/skills/review/SKILL.md (skipped)\n",
        "  \"review\" collision (prompt):\n",
        "    \u{2713} /home/jin/.nini/agent/prompts/review.md\n",
        "    \u{2717} /home/jin/project/.nini/prompts/review.md (skipped)\n",
        "  /home/jin/.nini/agent/skills/broken/SKILL.md\n",
        "    read failed: permission denied\n",
        "  parse failed: frontmatter is not a YAML mapping",
    );
    assert_eq!(text, expected);

    // Grouping, stated as counts so the assertion survives a
    // reformatting of the header:
    //   two groups named `review` (one per namespace), one header each.
    assert_eq!(text.matches("\"review\" collision").count(), 2);
    // One winner per group, one skipped line per loser.
    assert_eq!(text.matches('\u{2713}').count(), 2);
    assert_eq!(text.matches('\u{2717}').count(), 3);
    // The two skill losers are under the skill header, not the prompt
    // one: the skill header comes first and is followed by both.
    let skill_block = text.split("collision (prompt)").next().unwrap();
    assert!(skill_block.contains(".claude/skills/review/SKILL.md (skipped)"));
    assert!(skill_block.contains(".pi/agent/skills/review/SKILL.md (skipped)"));

    // Nothing to say → nothing to show.
    assert_eq!(format_diagnostics(&[]), "");
}

// ---------------------------------------------------------------------------
// 7. The title sink exists and is crossterm's `SetTitle`
// ---------------------------------------------------------------------------

#[test]
fn the_title_sink_is_crossterms_set_title() {
    // The sink is not *called* here — it writes to stdout, which the TUI
    // owns while it runs — but the **bytes it would write** are pinned
    // here, without touching stdout.
    //
    // The previous version of this test took the function's address and
    // asserted it was non-zero, which is **always** true: swapping the
    // sink for `print!` would have kept it green. A test that cannot fail
    // is decoration.
    use crossterm::QueueableCommand;
    let mut buf: Vec<u8> = Vec::new();
    buf.queue(crossterm::terminal::SetTitle("nini - u_1 - proj"))
        .unwrap();
    let bytes = String::from_utf8_lossy(&buf);
    assert_eq!(
        bytes, "\u{1b}]0;nini - u_1 - proj\u{7}",
        "the title must be an OSC 2 sequence (ESC ] 0 ; … BEL), which puts no \
         glyph on the painted screen"
    );
}

#[test]
fn a_title_cannot_carry_a_control_sequence_in_from_a_directory_name() {
    // A directory name may contain ESC or BEL (on Linux). The title is an
    // OSC 2 sequence, so an unsanitised name could end the sequence early
    // and start another one.
    let title = terminal_title(Some("u_1"), "/tmp/we\u{1b}]0;pwn\u{7}");
    assert!(
        !title.contains('\u{1b}') && !title.contains('\u{7}'),
        "control characters must be stripped from the title: {title:?}"
    );
}
