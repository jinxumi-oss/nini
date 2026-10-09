//! R2g: an image must not vanish without saying so.
//!
//! `entries::ContentBlock::Image` exists because R1f added it to parse real
//! Pi sessions. Every conversion to a provider message then dropped it:
//! `None` for user and assistant content, `String::new()` inside a tool
//! result. The image was gone before it reached any provider, with no word
//! to anyone.
//!
//! `provider::ContentBlock` has no image variant - Text, ToolUse, ToolResult
//! only - so an image genuinely cannot be sent today, and
//! `Capabilities::vision` is `false` for every provider, which is accurate.
//! The defect was the silence, not the absence.
//!
//! Pi puts a placeholder in its place (`transform-messages.ts:12-13`), and
//! those are the strings asserted here verbatim.

use nini_core::conversion::{
    session_entry_to_llm_message, NON_VISION_TOOL_IMAGE_PLACEHOLDER,
    NON_VISION_USER_IMAGE_PLACEHOLDER,
};
use nini_core::entries::{AgentMessage, ContentBlock, StringOrContentBlocks};

fn image_session_message() -> nini_core::SessionEntry {
    nini_core::SessionEntry::Message(nini_core::entries::SessionMessageEntry {
        id: "m1".into(),
        parent_id: None,
        timestamp: "2026-10-05T00:00:00.000Z".into(),
        message: AgentMessage::User(nini_core::entries::UserMessage {
            content: StringOrContentBlocks::Blocks(vec![
                ContentBlock::Text {
                    text: "what is in this?".into(),
                },
                ContentBlock::Image {
                    data: "iVBORw0KGgo=".into(),
                    mime_type: Some("image/png".into()),
                },
            ]),
            timestamp: 0,
        }),
    })
}

fn texts(msg: &nini_core::provider::Message) -> Vec<String> {
    msg.content
        .iter()
        .filter_map(|b| match b {
            nini_core::provider::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The headline. Before this the image produced no block at all; the
/// sentence either side of it stayed and the gap was invisible.
#[test]
fn a_user_image_becomes_pis_placeholder() {
    let msg = session_entry_to_llm_message(&image_session_message())
        .expect("a user message must convert");
    let got = texts(&msg);

    assert!(
        got.iter()
            .any(|t| t.contains(NON_VISION_USER_IMAGE_PLACEHOLDER)),
        "Pi replaces the image with {:?}; nini used to drop it silently, \
         so the model saw a sentence with a hole in it. Got {got:?}",
        NON_VISION_USER_IMAGE_PLACEHOLDER
    );
    assert_eq!(
        NON_VISION_USER_IMAGE_PLACEHOLDER,
        "(image omitted: model does not support images)"
    );
}

/// The surrounding text must survive alongside it. A placeholder that
/// replaced the whole message would be as broken as a silent drop.
#[test]
fn the_text_around_the_image_survives() {
    let msg = session_entry_to_llm_message(&image_session_message()).unwrap();
    let got = texts(&msg).join(" ");
    assert!(
        got.contains("what is in this?"),
        "the user question must not be lost to the placeholder: {got}"
    );
}

/// Pi's tool-image placeholder is a different string
/// (`transform-messages.ts:13`), and the two must not be conflated.
#[test]
fn a_tool_result_image_uses_pis_other_placeholder() {
    let entry = nini_core::SessionEntry::Message(nini_core::entries::SessionMessageEntry {
        id: "t1".into(),
        parent_id: None,
        timestamp: "2026-10-05T00:00:00.000Z".into(),
        message: AgentMessage::ToolResult(nini_core::entries::ToolResultMessage {
            tool_call_id: "c1".into(),
            tool_name: "read".into(),
            content: vec![
                ContentBlock::Text { text: "ok".into() },
                ContentBlock::Image {
                    data: "iVBORw0KGgo=".into(),
                    mime_type: Some("image/png".into()),
                },
            ],
            details: None,
            usage: None,
            is_error: false,
            timestamp: 0,
        }),
    });

    let msg = session_entry_to_llm_message(&entry).expect("must convert");
    let body = match &msg.content[0] {
        nini_core::provider::ContentBlock::ToolResult { content, .. } => content.clone(),
        other => panic!("expected a tool result, got {other:?}"),
    };

    assert!(
        body.contains(NON_VISION_TOOL_IMAGE_PLACEHOLDER),
        "Pi's tool placeholder is {:?}; got {body}",
        NON_VISION_TOOL_IMAGE_PLACEHOLDER
    );
    assert_ne!(
        NON_VISION_TOOL_IMAGE_PLACEHOLDER, NON_VISION_USER_IMAGE_PLACEHOLDER,
        "the two are different strings in Pi and must stay so"
    );
}

/// Thinking and tool-call blocks are still dropped, unchanged. Pinned so
/// that fixing the image case did not quietly change something else.
#[test]
fn thinking_and_tool_calls_are_still_dropped() {
    let entry = nini_core::SessionEntry::Message(nini_core::entries::SessionMessageEntry {
        id: "a1".into(),
        parent_id: None,
        timestamp: "2026-10-05T00:00:00.000Z".into(),
        message: AgentMessage::Assistant(nini_core::entries::AssistantMessage {
            content: vec![
                ContentBlock::Text {
                    text: "visible".into(),
                },
                ContentBlock::Thinking {
                    thinking: "hidden".into(),
                },
            ],
            api: "anthropic".into(),
            provider: "anthropic".into(),
            model: "claude".into(),
            usage: Default::default(),
            stop_reason: nini_core::entries::StopReason::Stop,
            error_message: None,
            timestamp: 0,
        }),
    });

    let msg = session_entry_to_llm_message(&entry).expect("must convert");
    let got = texts(&msg);
    assert_eq!(got, vec!["visible".to_string()], "{got:?}");
}

/// A message with no image is untouched by any of this.
#[test]
fn a_plain_text_message_is_unchanged() {
    let entry = nini_core::SessionEntry::Message(nini_core::entries::SessionMessageEntry {
        id: "m2".into(),
        parent_id: None,
        timestamp: "2026-10-05T00:00:00.000Z".into(),
        message: AgentMessage::User(nini_core::entries::UserMessage {
            content: StringOrContentBlocks::String("just words".into()),
            timestamp: 0,
        }),
    });

    let msg = session_entry_to_llm_message(&entry).unwrap();
    assert_eq!(texts(&msg), vec!["just words".to_string()]);
}
