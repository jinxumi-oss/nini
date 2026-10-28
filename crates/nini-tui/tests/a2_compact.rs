//! A2: `/compact` and auto-compaction must actually compact.
//!
//! Both used to be display-only. `cmd_compact` drained the first half of the
//! **transcript lines** at `total / 2`, built a numbered list of line
//! previews, and put it back into the transcript. `auto_compact_local` did
//! the same thing on the auto-trigger path. Neither wrote to the session,
//! neither consulted a token budget, and neither changed anything the model
//! would see. `cmd_compact`'s comment claimed it "matches the algorithm
//! used by the auto-compaction path" and called the cut `find_cut_point`,
//! which was never called from there.
//!
//! The cut is a token budget over the session now, and the result is a real
//! `CompactionEntry` that `build_context_entries` reconstructs from.
//!
//! These tests drive the command dispatcher, not the helpers, because the
//! dispatcher is where the fake lived.

use nini_tui::commands::{dispatch, CommandId, CommandOutcome};
use nini_tui::settings::SettingsManager;
use nini_tui::state::AppState;

fn state_with_session(tag: &str) -> (AppState, SettingsManager, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("nini-a2-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = AppState::new("test-model");
    s.session_init(dir.join("s.jsonl"));
    let mut settings = SettingsManager::default();
    settings.settings.keep_recent_tokens = 8;
    (s, settings, dir)
}

fn messages(s: &AppState) -> Vec<String> {
    let arc = s.session_state.session.as_ref().expect("session").clone();
    let g = nini_tui::state::lock_session(&arc);
    g.entries
        .iter()
        .map(|e| match e {
            nini_core::SessionEntry::Compaction(c) => format!("compaction:{}", c.summary),
            nini_core::SessionEntry::Message(_) => "message".to_string(),
            other => format!("other:{}", other.id()),
        })
        .collect()
}

/// The headline: `/compact` must leave a Compaction entry on disk. Before
/// this change it left the session byte-for-byte unchanged.
#[test]
fn compact_writes_a_real_compaction_entry() {
    let (mut s, mut settings, dir) = state_with_session("writes");
    for i in 0..10 {
        s.push_user(format!("question {i}: how to refactor auth?"));
        s.push_assistant(format!("answer {i}: use sqlx, not diesel."));
    }
    let before = messages(&s);
    assert!(
        !before.iter().any(|m| m.starts_with("compaction:")),
        "precondition: no compaction yet"
    );

    let r = dispatch(&mut s, &mut settings, CommandId::Compact, "");
    assert!(matches!(r.outcome, CommandOutcome::Output(_)));

    let after = messages(&s);
    assert!(
        after.iter().any(|m| m.starts_with("compaction:")),
        "`/compact` must record a CompactionEntry. It used to fold \
         transcript lines and write nothing: {after:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The cut must come from the token budget, not from `len / 2`.
///
/// A transcript of filler would cross `len / 2` easily while staying far
/// inside any sane token budget. The fake folded it anyway and threw the
/// content away; the real one leaves it alone.
#[test]
fn a_session_inside_the_token_budget_is_not_folded() {
    let (mut s, mut settings, dir) = state_with_session("budget");
    // Restore the default budget: 20k tokens, which 20 short exchanges
    // nowhere near reach.
    settings.settings.keep_recent_tokens = 20_000;
    for i in 0..10 {
        s.push_user(format!("q{i}"));
        s.push_assistant(format!("a{i}"));
    }
    let before = s.transcript_state.lines.len();

    let r = dispatch(&mut s, &mut settings, CommandId::Compact, "");
    match r.outcome {
        CommandOutcome::Output(lines) => assert!(
            lines.iter().any(|l| l.contains("nothing to compact")),
            "a session well inside its budget must be left alone: {lines:?}"
        ),
        _ => panic!("expected Output"),
    }
    assert!(
        !messages(&s).iter().any(|m| m.starts_with("compaction:")),
        "and nothing may be written"
    );
    assert_eq!(s.transcript_state.lines.len(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The transcript must be re-derived from the session's live context, so
/// what the user reads is what the agent will be given.
#[test]
fn the_transcript_is_rebuilt_from_the_session_context() {
    let (mut s, mut settings, dir) = state_with_session("rebuild");
    for i in 0..10 {
        s.push_user(format!("question {i}"));
        s.push_assistant(format!("answer {i}"));
    }
    dispatch(&mut s, &mut settings, CommandId::Compact, "");

    let head_is_summary = matches!(
        s.transcript_state.lines.first(),
        Some(nini_tui::state::TranscriptLine::AssistantText(t))
            if t.contains("[CONTEXT SUMMARY]")
    );
    assert!(
        head_is_summary,
        "the summary must lead the transcript, since the session's context \
         leads with the CompactionEntry"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The two triggers answer different questions, and each must use its own
/// real criterion.
///
/// `/compact` asks "is there anything to fold, given what I have to keep?"
/// and is bounded by `keep_recent_tokens`. Auto-compaction asks "is the
/// context near full?" and is bounded by `context_window - reserve_tokens`.
/// Pi keeps them separate too. They are not supposed to agree.
///
/// What was broken was not that they differed but that `/compact` used no
/// real criterion at all: `total / 2` over transcript lines. So this pins
/// each one against its own bound.
#[test]
fn each_trigger_uses_its_own_bound() {
    // Auto: a small context window with a normal keep budget. Manual:
    // a normal context window with a small keep budget.
    let (mut auto_s, auto_settings, auto_dir) = state_with_session("auto-bound");
    let (mut manual_s, _, manual_dir) = state_with_session("manual-bound");
    auto_s.run_state.context_window = 100;
    manual_s.run_state.context_window = 200_000;

    for s in [&mut auto_s, &mut manual_s] {
        for i in 0..10 {
            s.push_user(format!("q{i}"));
            s.push_assistant(format!("a{i}"));
        }
    }

    // Auto's bound: over budget for a 100-token window.
    assert!(
        auto_s.should_auto_compact(&auto_settings),
        "a 100-token window with 48 estimated tokens is over budget, so the \
         auto trigger must fire on its own criterion"
    );
    // Manual's bound: nothing to fold inside a 20k keep budget.
    let mut m_settings = SettingsManager::default();
    m_settings.settings.keep_recent_tokens = 20_000;
    manual_s.run_state.context_window = 200_000;
    let r = dispatch(&mut manual_s, &mut m_settings, CommandId::Compact, "");
    match r.outcome {
        CommandOutcome::Output(lines) => assert!(
            lines.iter().any(|l| l.contains("nothing to compact")),
            "the manual path must answer against keep_recent_tokens, not \
             against the context window and not against len/2: {lines:?}"
        ),
        _ => panic!("expected Output"),
    }

    let _ = std::fs::remove_dir_all(&auto_dir);
    let _ = std::fs::remove_dir_all(&manual_dir);
}

/// `firstKeptEntryId` must be the entry the cut actually landed on.
///
/// A first version compared the recorded id against the first entry of the
/// rebuilt context - which is circular, because the rebuild *uses* that id.
/// It therefore passed with the id pointing one entry too early. The
/// expected id is recomputed here from the algorithm, independently of
/// anything `Session::compact` recorded.
#[test]
fn first_kept_entry_id_is_the_entry_the_cut_landed_on() {
    use nini_core::compaction::find_cut_point;

    let (mut s, mut settings, dir) = state_with_session("first-kept");
    for i in 0..10 {
        s.push_user(format!("question {i}"));
        s.push_assistant(format!("answer {i}"));
    }

    // Independently: run the cut algorithm over the pre-compaction path and
    // keep the id it lands on.
    let expected = {
        let arc = s.session_state.session.as_ref().unwrap().clone();
        let g = nini_tui::state::lock_session(&arc);
        let legacy: Vec<nini_core::Entry> =
            g.get_path().into_iter().cloned().map(Into::into).collect();
        let cut = find_cut_point(&legacy, settings.settings.keep_recent_tokens);
        assert!(
            cut.keep_from > 0 && cut.keep_from < legacy.len(),
            "precondition: the cut must land strictly inside the session, \
             got keep_from={} of {}",
            cut.keep_from,
            legacy.len()
        );
        legacy[cut.keep_from].id.clone()
    };

    dispatch(&mut s, &mut settings, CommandId::Compact, "");

    let (recorded, ctx_len) = {
        let arc = s.session_state.session.as_ref().unwrap().clone();
        let g = nini_tui::state::lock_session(&arc);
        let cut = g
            .get_path()
            .into_iter()
            .find(|e| matches!(e, nini_core::SessionEntry::Compaction(_)))
            .expect("a compaction must be on the path");
        let nini_core::SessionEntry::Compaction(c) = cut else {
            unreachable!()
        };
        (
            c.first_kept_entry_id.clone(),
            nini_session::build_context_entries(&g).len(),
        )
    };

    assert_eq!(
        recorded.as_deref(),
        Some(expected.as_str()),
        "firstKeptEntryId must name the entry the cut landed on, \
         independently of what the rebuild does with it"
    );
    assert!(
        ctx_len >= 1,
        "and the rebuild must find it, or the context is empty"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
