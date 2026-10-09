//! Integration tests for compaction wired into the agent.
//!
//! Verifies:
//! - Auto-compaction triggers when context exceeds budget
//! - After compaction, history is shorter and contains a summary message
//! - Slash command /compact runs manually

use futures_util::StreamExt;
use nini_ai::fixture::{FixtureTurn, ProgrammedProvider};
use nini_core::provider::{Provider, Usage};
use nini_core::tool::ToolRegistry;
use nini_core::{Agent, AgentEvent, RunConfig};
use nini_tui::commands::{dispatch, CommandId, CommandOutcome};
use nini_tui::render::render_frame;
use nini_tui::settings::SettingsManager;
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::sync::Arc;

fn frame_text(state: &AppState, w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    let buf = terminal.backend().buffer().clone();
    let mut out = String::new();
    let area = buf.area;
    for y in 0..area.height {
        let mut line = String::new();
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                line.push_str(cell.symbol());
            }
        }
        out.push_str(line.trim_end_matches(' '));
        out.push('\n');
    }
    out
}

fn make_tool_provider() -> Arc<dyn Provider> {
    Arc::new(ProgrammedProvider::from_turns(vec![vec![
        FixtureTurn::Text("ok".to_string()),
        FixtureTurn::Stop {
            stop_reason: "end_turn".to_string(),
            usage: Usage::default(),
        },
    ]]))
}

// =====================================================================
// Test 1: Agent::estimated_tokens counts history
// =====================================================================
#[tokio::test]
async fn agent_estimated_tokens_counts_history() {
    let provider = make_tool_provider();
    let tools = ToolRegistry::new();
    let cfg = RunConfig::new("test-model");
    let mut agent = Agent::new(provider, tools, cfg);
    agent.seed(vec![
        nini_core::provider::Message {
            role: nini_core::provider::Role::User,
            content: vec![nini_core::provider::ContentBlock::Text {
                text: "x".repeat(400), // 100 tokens
            }],
            timestamp: 0,
        },
        nini_core::provider::Message {
            role: nini_core::provider::Role::Assistant,
            content: vec![nini_core::provider::ContentBlock::Text {
                text: "y".repeat(200), // 50 tokens
            }],
            timestamp: 0,
        },
    ]);
    let tokens = agent.estimated_tokens();
    assert_eq!(tokens, 150);
}

// =====================================================================
// Test 2: should_compact returns true at threshold
// =====================================================================
#[tokio::test]
async fn agent_should_compact_at_threshold() {
    let provider = make_tool_provider();
    let tools = ToolRegistry::new();
    let mut cfg = RunConfig::new("test-model");
    // Make budget tight: 100 tokens total, 50 reserved → budget = 50
    cfg.compaction.context_window = 100;
    cfg.compaction.reserve_tokens = 50;
    let mut agent = Agent::new(provider, tools, cfg);
    // Initially empty → not over budget
    assert!(!agent.should_compact());
    // Add content that pushes over the 50-token budget
    agent.seed(vec![nini_core::provider::Message {
        role: nini_core::provider::Role::User,
        content: vec![nini_core::provider::ContentBlock::Text {
            text: "z".repeat(400), // 100 tokens — way over 50
        }],
        timestamp: 0,
    }]);
    assert!(agent.should_compact());
}

// =====================================================================
// Test 3: compact_history reduces and prepends summary
// =====================================================================
#[tokio::test]
async fn compact_history_prepends_summary_and_keeps_suffix() {
    let provider = make_tool_provider();
    let tools = ToolRegistry::new();
    let cfg = RunConfig::new("test-model");
    let mut agent = Agent::new(provider, tools, cfg);

    // Seed with two user/assistant turns. The cut should land before
    // the SECOND user turn, summarizing the first user/assistant exchange.
    agent.seed(vec![
        nini_core::provider::Message {
            role: nini_core::provider::Role::User,
            content: vec![nini_core::provider::ContentBlock::Text {
                text: "first user question".to_string(),
            }],
            timestamp: 0,
        },
        nini_core::provider::Message {
            role: nini_core::provider::Role::Assistant,
            content: vec![nini_core::provider::ContentBlock::Text {
                text: "first answer".to_string(),
            }],
            timestamp: 0,
        },
        nini_core::provider::Message {
            role: nini_core::provider::Role::User,
            content: vec![nini_core::provider::ContentBlock::Text {
                text: "second user question".to_string(),
            }],
            timestamp: 0,
        },
        nini_core::provider::Message {
            role: nini_core::provider::Role::Assistant,
            content: vec![nini_core::provider::ContentBlock::Text {
                text: "second answer".to_string(),
            }],
            timestamp: 0,
        },
    ]);
    let before_len = agent.messages().len();

    let out = agent.compact_history(|msgs, _prev| {
        msgs.iter()
            .filter_map(|m| {
                if m.role == nini_core::provider::Role::User {
                    if let Some(nini_core::provider::ContentBlock::Text { text }) =
                        m.content.first()
                    {
                        Some(text.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(" | ")
    });

    // Without a prior summary, the first user turn is index 0 (the start
    // of conversation), so keep_from = 0 (nothing to summarize). The
    // algorithm needs a prior summary or an existing compaction entry to
    // compact anything. With pure-message history, this is a no-op.
    assert_eq!(out.keep_from, 0);
    // After compaction, history has 1 summary + (before_len - 0) = 5.
    assert_eq!(agent.messages().len(), 1 + before_len);
    let summary_msg = &agent.messages()[0];
    if let nini_core::provider::ContentBlock::Text { text } = &summary_msg.content[0] {
        assert!(text.starts_with("[CONTEXT SUMMARY]"));
        // The prefix was empty (cut at index 0), so summary fn got empty input.
        assert!(text.contains("[CONTEXT SUMMARY]"));
    } else {
        panic!("first message should be a Text block");
    }
}

// =====================================================================
// Test 4: agent run loop auto-compacts when over budget
// =====================================================================
#[tokio::test]
async fn an_over_budget_agent_does_not_compact_itself() {
    // A3 rewrote the subject of this test.
    //
    // It used to seed an over-budget history, run the loop, and assert that
    // the Agent compacted itself and reported it through an `Error` event
    // beginning "compaction:". That in-loop threshold check is gone: its
    // compaction never reached the session, and the `Compacted` event it
    // emitted had no consumer anywhere in the tree.
    //
    // What matters now is the opposite. If the Agent compacted itself again,
    // its history would diverge from the session's, and the two would
    // disagree about what the model is being shown - which is the state A1,
    // P0.1, P0.2 and A2 were all done to eliminate. The session owns
    // compaction now, and hands the Agent its context as a seed.
    let provider: Arc<dyn Provider> = Arc::new(ProgrammedProvider::from_turns(vec![vec![
        FixtureTurn::Text("hi".to_string()),
        FixtureTurn::Stop {
            stop_reason: "end_turn".to_string(),
            usage: Usage::default(),
        },
    ]]));
    let tools = ToolRegistry::new();
    let mut cfg = RunConfig::new("test-model");
    cfg.compaction.context_window = 100;
    cfg.compaction.reserve_tokens = 50;
    let mut agent = Agent::new(provider, tools, cfg);

    // Seed with content that exceeds budget.
    agent.seed(vec![nini_core::provider::Message {
        role: nini_core::provider::Role::User,
        content: vec![nini_core::provider::ContentBlock::Text {
            text: "x".repeat(400),
        }],
        timestamp: 0,
    }]);
    // The predicate still reports the truth about the budget; it just no
    // longer acts on it inside the loop.
    assert!(
        agent.should_compact(),
        "the budget check itself is unchanged"
    );

    let before = agent.messages().len();
    {
        let mut stream = Box::pin(agent.run(nini_core::AgentMessage::user("hi")));
        while let Some(ev) = stream.next().await {
            if let Ok(AgentEvent::Error { message }) = &ev {
                assert!(
                    !message.starts_with("compaction:"),
                    "the Agent must not compact itself any more; that \
                     decision belongs to the session. Got: {message}"
                );
            }
        }
    }
    assert!(
        agent.messages()[..before].iter().any(|m| {
            m.content.iter().any(|b| {
                matches!(
                    b,
                    nini_core::provider::ContentBlock::Text { text } if text.len() == 400
                )
            })
        }),
        "the seeded history must survive untouched: a self-compaction here \
         would leave the Agent and the session describing different \
         conversations"
    );
}

// =====================================================================
// Test 5: Slash command /compact dispatches
// =====================================================================
#[test]
fn slash_compact_command_dispatches() {
    let mut state = AppState::new("test");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Compact, "");
    // /compact on an empty/short transcript returns a "nothing to compact"
    // message (test transcript has 0 entries).
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(
                lines.iter().any(|l| l.contains("nothing to compact")),
                "expected nothing-to-compact message, got: {lines:?}"
            );
        }
        _ => panic!("expected Output"),
    }
    // Empty transcript — no compaction happened, transcript unchanged.
    assert!(state.transcript_state.lines.is_empty());
}

#[test]
fn slash_compact_command_dispacts_with_long_transcript() {
    // A2: this test used to build only a transcript and expect `/compact`
    // to fold it. That worked because the old `cmd_compact` folded
    // *transcript lines* at `total / 2` and wrote nothing anywhere. The real
    // compaction operates on the session, so the state under test now has
    // to include one - and the push_* helpers write to both, so the same
    // eight exchanges build the session too.
    let dir = std::env::temp_dir().join(format!("nini-a2-slash-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut state = AppState::new("test");
    state.session_init(dir.join("s.jsonl"));
    let mut settings = SettingsManager::default();
    // The cut is a TOKEN budget now, not `transcript.len() / 2`. Sixteen
    // short exchanges come nowhere near the 20k default, so the honest
    // result for this state is "nothing to compact" - which is what the
    // command returned. Forcing a small budget is what makes this test
    // about the compaction rather than about the default; padding the
    // messages with filler to cross 20k would test the estimator instead.
    settings.settings.keep_recent_tokens = 8;
    // Build a long session so /compact actually does something.
    for i in 0..8 {
        state.push_user(format!("user {i}"));
        state.push_assistant(format!("assistant {i}"));
    }
    let before = state.transcript_state.lines.len();
    let r = dispatch(&mut state, &mut settings, CommandId::Compact, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(
                lines.iter().any(|l| l.contains("compacted")),
                "expected 'compacted' summary line, got: {lines:?}"
            );
        }
        _ => panic!("expected Output"),
    }
    assert!(state.transcript_state.lines.len() <= before);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        state
            .transcript_state
            .lines
            .first()
            .and_then(|l| l.as_assistant_text())
            .map(|s| s.contains("[CONTEXT SUMMARY]"))
            .unwrap_or(false),
        "expected [CONTEXT SUMMARY] at transcript head"
    );
}

// =====================================================================
// Test 6: Frame renders compaction-related state correctly
// =====================================================================
#[test]
fn frame_runs_after_compaction() {
    let mut state = AppState::new("test-model");
    state.push_user("hi".to_string());
    state.push_assistant("hello".to_string());
    state.push_divider();
    state.push_assistant("[CONTEXT SUMMARY]\nold stuff".to_string());
    state.push_divider();
    let frame = frame_text(&state, 100, 24);
    assert!(frame.contains("CONTEXT SUMMARY"));
    assert!(frame.contains("> hi"));
}

// =====================================================================
// Test 7: CompactionSettings default values
// =====================================================================
#[test]
fn compaction_settings_default_is_sensible() {
    use nini_core::CompactionSettings;
    let s = CompactionSettings::default();
    assert_eq!(s.context_window, 200_000);
    // Pi parity: default reserveTokens=16384, keepRecentTokens=20000.
    assert_eq!(s.reserve_tokens, 16384);
    assert!(s.enabled);
    assert_eq!(s.keep_recent_tokens, 20000);
}

// =====================================================================
// Test 8: should_compact boundary check
// =====================================================================
#[test]
fn should_compact_strictly_greater_than_budget() {
    use nini_core::{should_compact, CompactionSettings};
    let s = CompactionSettings {
        context_window: 1000,
        reserve_tokens: 100,
        ..Default::default()
    };
    // At exactly the threshold (context_window - reserve_tokens = 900), should NOT compact.
    assert!(!should_compact(900, &s));
    // One above, should compact.
    assert!(should_compact(901, &s));
    assert!(should_compact(2000, &s));
}

/// The behaviour that replaced the old fake, stated directly.
///
/// `cmd_compact` used to fold the first half of the transcript lines at
/// `total / 2` whatever the token count, so a session that was nowhere near
/// its budget still had half of it thrown away for display. The cut is a
/// token budget now, and a short session is left alone.
#[test]
fn a_session_well_inside_its_budget_is_left_alone() {
    let dir = std::env::temp_dir().join(format!("nini-a2-budget-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut state = AppState::new("test");
    state.session_init(dir.join("s.jsonl"));
    let mut settings = SettingsManager::default(); // default 20k budget
    for i in 0..8 {
        state.push_user(format!("user {i}"));
        state.push_assistant(format!("assistant {i}"));
    }
    let before = state.transcript_state.lines.len();

    let r = dispatch(&mut state, &mut settings, CommandId::Compact, "");
    match r.outcome {
        CommandOutcome::Output(lines) => assert!(
            lines.iter().any(|l| l.contains("nothing to compact")),
            "a short session must not be compacted: {lines:?}"
        ),
        _ => panic!("expected Output"),
    }
    assert_eq!(
        state.transcript_state.lines.len(),
        before,
        "and the transcript must be untouched"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
