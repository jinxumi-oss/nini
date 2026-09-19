//! Integration tests for the agent loop with a real `BashTool`.
//!
//! These tests exercise the full path: provider → agent → tool registry →
//! bash → process spawn → stdout capture → back to provider.

use futures_util::StreamExt;
use nini_ai::fixture::{FixtureTurn, ProgrammedProvider};
use nini_core::provider::Usage;
use nini_core::tool::ToolRegistry;
use nini_core::{Agent, AgentEvent, RunConfig};
use nini_tools::BashTool;
use std::sync::Arc;

#[tokio::test]
async fn demo_use_bash_to_print_hello() {
    let provider: Arc<dyn nini_core::Provider> = Arc::new(ProgrammedProvider::from_turns(vec![
        vec![
            FixtureTurn::ToolCall {
                name: "bash".to_string(),
                args: serde_json::json!({"command": "echo hello"}),
            },
            FixtureTurn::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::Text("hello".to_string()),
            FixtureTurn::Stop {
                stop_reason: "end_turn".to_string(),
                usage: Usage::default(),
            },
        ],
    ]));

    let tools = ToolRegistry::new().register(Arc::new(BashTool::new()));
    let mut agent = Agent::new(provider, tools, RunConfig::new("test-model"));

    let mut stream = Box::pin(agent.run(nini_core::AgentMessage::user("use bash to print hello")));

    let mut text = String::new();
    let mut saw_tool_call = false;
    let mut saw_tool_result = false;
    let mut tool_result_content = String::new();
    let mut saw_turn_ends = 0;

    while let Some(ev) = stream.next().await {
        match ev {
            Ok(AgentEvent::TextDelta { text: t }) => text.push_str(&t),
            Ok(AgentEvent::ToolCallStart { name, .. }) if name == "bash" => saw_tool_call = true,
            Ok(AgentEvent::ToolResult { output, .. }) => {
                saw_tool_result = true;
                tool_result_content = output.content;
            }
            Ok(AgentEvent::TurnEnd { .. }) => saw_turn_ends += 1,
            _ => {}
        }
    }

    assert!(saw_tool_call, "agent should have invoked bash tool");
    assert!(saw_tool_result, "agent should have received a tool result");
    assert!(
        tool_result_content.contains("hello"),
        "bash output should contain 'hello', got: {tool_result_content:?}"
    );
    assert_eq!(
        saw_turn_ends, 2,
        "should have completed 2 turns (tool then final)"
    );
    assert_eq!(text, "hello", "final assistant text should be 'hello'");
}

#[tokio::test]
async fn bash_nonzero_exit_marks_error() {
    let provider: Arc<dyn nini_core::Provider> = Arc::new(ProgrammedProvider::from_turns(vec![
        vec![
            FixtureTurn::ToolCall {
                name: "bash".to_string(),
                args: serde_json::json!({"command": "exit 7"}),
            },
            FixtureTurn::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::Text("failed".to_string()),
            FixtureTurn::Stop {
                stop_reason: "end_turn".to_string(),
                usage: Usage::default(),
            },
        ],
    ]));
    let tools = ToolRegistry::new().register(Arc::new(BashTool::new()));
    let mut agent = Agent::new(provider, tools, RunConfig::new("test-model"));
    let mut stream = Box::pin(agent.run(nini_core::AgentMessage::user("run exit 7")));

    let mut saw_error_result = false;
    while let Some(ev) = stream.next().await {
        if let Ok(AgentEvent::ToolResult { output, .. }) = ev {
            if output.is_error {
                saw_error_result = true;
            }
        }
    }
    assert!(
        saw_error_result,
        "non-zero exit should produce is_error tool result"
    );
}

#[tokio::test]
async fn agent_text_only_skips_tool_execution() {
    let provider: Arc<dyn nini_core::Provider> =
        Arc::new(ProgrammedProvider::from_turns(vec![vec![
            FixtureTurn::Text("just text".to_string()),
            FixtureTurn::Stop {
                stop_reason: "end_turn".to_string(),
                usage: Usage::default(),
            },
        ]]));
    let tools = ToolRegistry::new().register(Arc::new(BashTool::new()));
    let mut agent = Agent::new(provider, tools, RunConfig::new("test-model"));
    let mut stream = Box::pin(agent.run(nini_core::AgentMessage::user("hi")));

    let mut text = String::new();
    let mut saw_tool_call = false;
    let mut turn_count = 0;
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(AgentEvent::TextDelta { text: t }) => text.push_str(&t),
            Ok(AgentEvent::ToolCallStart { .. }) => saw_tool_call = true,
            Ok(AgentEvent::TurnEnd { .. }) => turn_count += 1,
            _ => {}
        }
    }
    assert_eq!(text, "just text");
    assert!(
        !saw_tool_call,
        "no tool calls expected when model returns text only"
    );
    assert_eq!(turn_count, 1);
}
