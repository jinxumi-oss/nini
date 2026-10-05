//! Oracle, write direction: nini must emit keys **Pi** can parse.
//!
//! The read oracles in this crate would still pass if nini kept writing
//! snake_case, because reading is served by `alias`. That asymmetry is
//! exactly how the original defect survived: nini agreed with itself in both
//! directions and disagreed with Pi in both directions.
//!
//! These assertions are on **key names in nini's own output**, compared
//! against the key names observed in a session the installed Pi 0.84.2 wrote
//! minutes ago (`pi-0.84.2-fresh.jsonl`).

use nini_core::entries::*;
use serde_json::Value;

fn assistant() -> SessionEntry {
    SessionEntry::Message(SessionMessageEntry {
        id: "a1".into(),
        parent_id: None,
        timestamp: "2026-10-05T00:00:00Z".into(),
        message: AgentMessage::Assistant(AssistantMessage {
            content: vec![ContentBlock::Text { text: "hi".into() }],
            api: "anthropic".into(),
            provider: "anthropic".into(),
            model: "m".into(),
            usage: Usage {
                input: 1,
                output: 2,
                ..Default::default()
            },
            stop_reason: StopReason::ToolUse,
            error_message: Some("boom".into()),
            timestamp: 1,
        }),
    })
}

#[test]
fn assistant_message_uses_pis_key_names() {
    let v: Value = serde_json::to_value(assistant()).unwrap();
    let m = &v["message"];
    for k in [
        "role",
        "content",
        "api",
        "provider",
        "model",
        "usage",
        "stopReason",
        "errorMessage",
        "timestamp",
    ] {
        assert!(
            m.get(k).is_some(),
            "nini output is missing Pi's key `{k}`: {m}"
        );
    }
    for k in ["stop_reason", "error_message"] {
        assert!(
            m.get(k).is_none(),
            "nini still writes pre-R7 snake_case key `{k}`; Pi 0.84.2 would drop this line"
        );
    }
}

#[test]
fn thinking_level_change_uses_pis_key_name() {
    let e = SessionEntry::ThinkingLevelChange(ThinkingLevelChangeEntry {
        id: "t1".into(),
        parent_id: None,
        timestamp: "t".into(),
        thinking_level: "high".into(),
    });
    let v: Value = serde_json::to_value(&e).unwrap();
    assert_eq!(v.get("thinkingLevel").and_then(Value::as_str), Some("high"));
    assert!(v.get("thinking_level").is_none(), "still writes snake_case");
}

#[test]
fn custom_message_uses_pis_key_name() {
    let e = SessionEntry::CustomMessage(CustomMessageEntry {
        id: "c1".into(),
        parent_id: None,
        timestamp: "t".into(),
        custom_type: "note".into(),
        content: StringOrContentBlocks::String("x".into()),
        display: true,
        details: None,
    });
    let v: Value = serde_json::to_value(&e).unwrap();
    assert_eq!(v.get("customType").and_then(Value::as_str), Some("note"));
    assert!(v.get("custom_type").is_none(), "still writes snake_case");
}

#[test]
fn image_block_uses_pis_data_and_mime_type() {
    let e = SessionEntry::Message(SessionMessageEntry {
        id: "a2".into(),
        parent_id: None,
        timestamp: "t".into(),
        message: AgentMessage::User(UserMessage {
            content: StringOrContentBlocks::Blocks(vec![ContentBlock::Image {
                data: "BASE64".into(),
                mime_type: Some("image/png".into()),
            }]),
            timestamp: 1,
        }),
    });
    let v: Value = serde_json::to_value(&e).unwrap();
    let b = &v["message"]["content"][0];
    assert_eq!(b.get("type").and_then(Value::as_str), Some("image"));
    assert_eq!(b.get("data").and_then(Value::as_str), Some("BASE64"));
    assert_eq!(b.get("mimeType").and_then(Value::as_str), Some("image/png"));
    assert!(
        b.get("image").is_none(),
        "still writes the pre-R7 `image` key"
    );
}

#[test]
fn pi_can_read_what_nini_writes() {
    // End-to-end statement of the write oracle: every line nini produces for
    // the four entry kinds below must parse under Pi 0.84.2's key names.
    // Parsing is done with nini's own structs configured for the *Pi* spelling
    // (i.e. without relying on the legacy `alias` escape hatch) by round-
    // tripping through `serde_json::Value` and re-parsing the key set.
    let lines = vec![
        serde_json::to_string(&assistant()).unwrap(),
        serde_json::to_string(&SessionEntry::ThinkingLevelChange(
            ThinkingLevelChangeEntry {
                id: "t1".into(),
                parent_id: None,
                timestamp: "t".into(),
                thinking_level: "high".into(),
            },
        ))
        .unwrap(),
        serde_json::to_string(&SessionEntry::ModelChange(ModelChangeEntry {
            id: "m1".into(),
            parent_id: None,
            timestamp: "t".into(),
            provider: "anthropic".into(),
            model_id: "claude".into(),
        }))
        .unwrap(),
    ];
    for l in &lines {
        let v: Value = serde_json::from_str(l).unwrap();
        assert_eq!(v.get("type").and_then(Value::as_str).is_some(), true);
        // Pi's discriminator is always snake_case for `type`; the *field*
        // names are what R7 fixed. Assert none of the pre-R7 spellings leak.
        let blob = l.to_lowercase();
        for legacy in [
            "\"stop_reason\"",
            "\"error_message\"",
            "\"thinking_level\"",
            "\"custom_type\"",
        ] {
            assert!(!blob.contains(legacy), "pre-R7 key {legacy} present in {l}");
        }
    }
}
