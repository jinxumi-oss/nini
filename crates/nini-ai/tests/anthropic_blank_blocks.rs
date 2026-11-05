//! An assistant turn that carries nothing but a blank text block is a
//! shape Anthropic rejects — Pi drops the whole turn, so nini must too.
//!
//! ## What was broken
//!
//! nini filtered blank text blocks out of a **user** turn (a user turn
//! with no text is a `400`: `content: []` is a `400`), but kept an
//! **assistant** turn that consisted of nothing but a blank text block:
//!
//! ```json
//! {"role":"assistant","content":[{"type":"text","text":""}]}
//! ```
//!
//! The comment in `anthropic.rs` claimed Pi behaves the same way, and
//! quoted `anthropic-messages.ts:1324` (`if (blocks.length === 0)
//! continue;`). But Pi's `blocks` at that point is the **filtered**
//! array: the blank text block was already skipped a few lines above.
//!
//! ## Pi's rule (verified against the source, not the comment)
//!
//! `references/pi-0.85.1/src/api/anthropic-messages.ts`:
//!
//! ```text
//! 1276        } else if (msg.role === "assistant") {
//! 1277            const blocks: ContentBlockParam[] = [];
//! 1278
//! 1279            for (const block of msg.content) {
//! 1280                if (block.type === "text") {
//! 1281                    if (block.text.trim().length === 0) continue;
//! 1282                    blocks.push({
//! 1283                        type: "text",
//! 1284                        text: sanitizeSurrogates(block.text),
//! 1285                    });
//! 1286                } else if (block.type === "thinking") {
//! ...
//! 1321                } else if (block.type === "toolCall") {
//! 1322                    blocks.push({
//! 1323                        type: "tool_use",
//! 1324                        id: block.id,
//! 1325                        name: isOAuthToken ? toClaudeCodeName(block.name) : block.name,
//! 1326                        input: block.arguments ?? {},
//! 1327                    });
//! 1328                }
//! 1329            }
//! 1330            if (blocks.length === 0) continue;
//! ```
//!
//! So: a blank text block is skipped (`:1281`), a `toolCall` becomes a
//! `tool_use` block (`:1321-1327`), and the turn is dropped when what
//! is left is empty (`:1330`). The blank text block is gone **before**
//! the length check — that is what makes the difference.
//!
//! ## What these tests pin
//!
//! * An assistant turn with only blank text is dropped **whole**.
//! * An assistant turn with a `tool_use` and a blank text block keeps
//!   its `tool_use` — the tool_use / tool_result pairing must survive.
//! * The user turn keeps its own rule (blank text dropped, turn
//!   dropped when nothing is left).

use nini_ai::anthropic::build_request_body;
use nini_core::provider::{ContentBlock, Message, Request, Role};
use serde_json::Value;

fn request(messages: Vec<Message>) -> Request {
    Request {
        model: "test-model".to_string(),
        messages,
        tools: vec![],
        max_tokens: Some(256),
        temperature: None,
        system: None,
        reasoning: None,
    }
}

fn text_message(role: Role, text: &str) -> Message {
    Message {
        role,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        timestamp: 0,
    }
}

/// The `messages` array of the wire body.
fn wire_messages(req: &Request) -> Vec<Value> {
    let body = build_request_body(req).expect("the request body must build");
    let value: Value = serde_json::from_str(&body).expect("the wire body must be JSON");
    value["messages"]
        .as_array()
        .expect("the wire body must carry a messages array")
        .clone()
}

/// The headline case: an assistant turn that is nothing but a blank
/// text block is dropped whole — exactly as Pi drops it.
#[test]
fn an_assistant_turn_with_only_blank_text_is_dropped() {
    let req = request(vec![
        text_message(Role::User, "hello"),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "   \n ".to_string(),
            }],
            timestamp: 0,
        },
        text_message(Role::User, "again"),
    ]);

    let messages = wire_messages(&req);

    assert!(
        !messages.iter().any(|m| m["role"] == "assistant"),
        "the blank assistant turn was kept: {messages:?}"
    );
    assert_eq!(
        messages.len(),
        2,
        "only the two user turns may survive: {messages:?}"
    );
}

/// The same for an *empty* text block — the shape the review measured
/// on the wire: `{"role":"assistant","content":[{"type":"text","text":""}]}`.
#[test]
fn an_assistant_turn_with_an_empty_text_block_is_dropped() {
    let req = request(vec![
        text_message(Role::User, "hello"),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: String::new(),
            }],
            timestamp: 0,
        },
    ]);

    let messages = wire_messages(&req);

    assert!(
        !messages.iter().any(|m| m["role"] == "assistant"),
        "the empty assistant turn was kept: {messages:?}"
    );
    assert_eq!(
        messages.len(),
        1,
        "only the user turn may survive: {messages:?}"
    );
}

/// The pairing must survive: an assistant turn that carries a
/// `tool_use` is kept even when its text is blank. Dropping it would
/// orphan the `tool_result` that follows.
#[test]
fn an_assistant_turn_with_a_tool_use_keeps_its_tool_use() {
    let req = request(vec![
        text_message(Role::User, "run it"),
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "   ".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "echo hi"}),
                },
            ],
            timestamp: 0,
        },
        Message {
            role: Role::Tool,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "hi".to_string(),
                is_error: false,
            }],
            timestamp: 0,
        },
    ]);

    let messages = wire_messages(&req);

    let assistant = messages
        .iter()
        .find(|m| m["role"] == "assistant")
        .unwrap_or_else(|| panic!("the assistant turn was dropped: {messages:?}"));

    let content = assistant["content"]
        .as_array()
        .expect("the assistant turn must carry content");
    assert_eq!(
        content.len(),
        1,
        "the blank text block must be gone and the tool_use kept: {messages:?}"
    );
    assert_eq!(content[0]["type"], "tool_use");
    assert_eq!(content[0]["id"], "call_1");
    assert_eq!(content[0]["name"], "bash");

    // The tool result must still be there, and still point at the call.
    let tool_turn = messages
        .iter()
        .find(|m| m["content"][0]["type"] == "tool_result")
        .unwrap_or_else(|| panic!("the tool result was dropped: {messages:?}"));
    assert_eq!(tool_turn["content"][0]["tool_use_id"], "call_1");
}

/// An assistant turn with real text and a tool call keeps both.
#[test]
fn an_assistant_turn_with_text_and_a_tool_use_keeps_both() {
    let req = request(vec![
        text_message(Role::User, "run it"),
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "running".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({}),
                },
            ],
            timestamp: 0,
        },
    ]);

    let messages = wire_messages(&req);
    let assistant = messages
        .iter()
        .find(|m| m["role"] == "assistant")
        .unwrap_or_else(|| panic!("the assistant turn was dropped: {messages:?}"));
    let content = assistant["content"].as_array().expect("content");
    assert_eq!(content.len(), 2, "both blocks must survive: {messages:?}");
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[1]["type"], "tool_use");
}

/// The user turn keeps Pi's own rule: blank text is dropped, and the
/// turn is dropped when nothing is left.
#[test]
fn a_user_turn_with_only_blank_text_is_dropped() {
    let req = request(vec![
        text_message(Role::User, "hello"),
        text_message(Role::User, "  "),
        text_message(Role::User, "again"),
    ]);

    let messages = wire_messages(&req);

    assert_eq!(
        messages.len(),
        2,
        "the blank user turn must be dropped: {messages:?}"
    );
    assert!(
        messages.iter().all(|m| m["role"] == "user"),
        "no assistant turn may appear: {messages:?}"
    );
}
