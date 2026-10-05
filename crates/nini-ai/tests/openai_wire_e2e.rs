//! End-to-end wire-format tests for the OpenAI provider.
//!
//! **Why this file exists.** v0.8.9's end-to-end run against a real
//! MiniMax-M3 found that `--provider openai` / `openai-compat` produced
//! *no assistant text at all*: the model ran tools perfectly and said
//! nothing — no error, exit code 0, empty stdout.
//!
//! Root cause: v0.8.4 fixed a real bug (a chunk carrying both a
//! `</think>` close and a tool-call start lost the tool call, because
//! the text handler did `return Some(fragment)` and skipped the
//! tool_calls loop) by stashing the text fragments in
//! `StreamState::fragments_pending` instead. **The drain that stash was
//! waiting for was never written.** The field had a writer and no
//! reader, so every `delta.content` was parsed, run through the think
//! filter, and dropped.
//!
//! 897 tests did not catch it because every one of them either used the
//! `fixture` provider (which never touches SSE parsing) or hand-built
//! `SseEvent` values and asserted on **tool-call** events. Nobody
//! asserted that `TextDelta` comes out of a real wire payload.
//!
//! These tests go through the real `stream()` — real HTTP, real SSE
//! bytes, real `translate_sse`, real `finalize` — so a regression in any
//! layer of that chain shows up here.

#![allow(clippy::needless_return)]

use futures_core::Stream;
use futures_util::StreamExt;
use nini_ai::openai_compat::OpenAiCompatProvider;
use nini_core::provider::{ContentBlock, Message, Provider, Request, Role, StreamEvent};
use std::pin::Pin;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Serve `chunks` as a standard OpenAI SSE stream and run one request
/// through the provider, returning every event it produced.
async fn events_from_sse(chunks: &[&str]) -> Vec<StreamEvent> {
    let server = MockServer::start().await;
    let mut body = String::new();
    for c in chunks {
        body.push_str("data: ");
        body.push_str(c);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let provider = OpenAiCompatProvider::new(server.uri(), "test-key");
    let req = Request {
        model: "test-model".to_string(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
            timestamp: 0,
        }],
        tools: vec![],
        max_tokens: Some(256),
        temperature: None,
        system: None,
    };

    let mut stream: Pin<
        Box<dyn Stream<Item = Result<StreamEvent, nini_core::provider::ProviderError>> + Send>,
    > = provider.stream(req);
    let mut out = Vec::new();
    while let Some(ev) = stream.next().await {
        out.push(ev.expect("provider yielded an error event"));
    }
    out
}

/// Concatenate every TextDelta in order.
fn visible_text(events: &[StreamEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn thinking_text(events: &[StreamEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ThinkingDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn has_stop(events: &[StreamEvent]) -> bool {
    matches!(events.last(), Some(StreamEvent::MessageStop { .. }))
}

// ---------------------------------------------------------------------------

/// The headline regression. A plain text response must reach the caller.
#[tokio::test]
async fn plain_text_response_reaches_the_caller() {
    let events = events_from_sse(&[
        r#"{"choices":[{"delta":{"role":"assistant"},"finish_reason":"","index":0}],"usage":null}"#,
        r#"{"choices":[{"delta":{"content":"Hello, world!"},"finish_reason":"","index":0}],"usage":null}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":9,"completion_tokens":3}}"#,
    ])
    .await;

    assert_eq!(
        visible_text(&events),
        "Hello, world!",
        "assistant text was dropped on the floor. events: {events:?}"
    );
    assert!(has_stop(&events), "stream must end with MessageStop");
}

/// Many small deltas — the normal shape of a real response. Every one of
/// them has to survive, in order.
#[tokio::test]
async fn every_delta_in_a_multi_chunk_response_survives_in_order() {
    let events = events_from_sse(&[
        r#"{"choices":[{"delta":{"content":"The "},"index":0}]}"#,
        r#"{"choices":[{"delta":{"content":"quick "},"index":0}]}"#,
        r#"{"choices":[{"delta":{"content":"brown "},"index":0}]}"#,
        r#"{"choices":[{"delta":{"content":"fox"},"index":0}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#,
    ])
    .await;

    assert_eq!(
        visible_text(&events),
        "The quick brown fox",
        "deltas were lost or reordered"
    );
}

/// **Streaming, not just "the text arrives eventually".**
///
/// This is a different assertion from the ones above and it is the one
/// that actually pins the bug. Removing the per-call drain in
/// `translate_sse` still leaves every character intact, because
/// `finalize` flushes the whole queue at stream end — the first version
/// of this file passed with the drain deleted.
///
/// That is not a cosmetic difference:
///   * the TUI has no typewriter effect; a long answer appears all at
///     once when generation finishes,
///   * and if the run is aborted mid-answer, every character received
///     so far is discarded, because they were all still sitting in the
///     queue.
///
/// So: with 3 content chunks, we must see 3 separate `TextDelta` events
/// ahead of the `MessageStop`.
#[tokio::test]
async fn text_arrives_incrementally_not_all_at_stream_end() {
    let events = events_from_sse(&[
        r#"{"choices":[{"delta":{"content":"one "},"index":0}]}"#,
        r#"{"choices":[{"delta":{"content":"two "},"index":0}]}"#,
        r#"{"choices":[{"delta":{"content":"three"},"index":0}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#,
    ])
    .await;

    let deltas: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta { text } => Some(text),
            _ => None,
        })
        .collect();
    let stop_pos = events
        .iter()
        .position(|e| matches!(e, StreamEvent::MessageStop { .. }))
        .expect("no MessageStop");

    assert_eq!(
        deltas,
        vec!["one ", "two ", "three"],
        "expected one TextDelta per content chunk, got {deltas:?}"
    );

    // The decisive one: all three landed *before* the turn ended.
    let last_delta = events
        .iter()
        .rposition(|e| matches!(e, StreamEvent::TextDelta { .. }))
        .unwrap();
    assert!(
        last_delta < stop_pos,
        "text was still queued when the turn ended — it will all be lost \
         if the run is aborted: {events:?}"
    );
}

/// The ordering that actually distinguishes "drained per chunk" from
/// "flushed at the end".
///
/// Counting events cannot tell them apart: `finalize` also emits one
/// event per fragment, so a 3-chunk response produces 3 `TextDelta`
/// either way. **Order** can.
///
/// Wiremock hands the whole body over in one TCP write, so reqwest may
/// deliver it as a single byte-chunk and the parser may hand
/// `translate_sse` all four events back to back. The two behaviours
/// still differ in what the user sees:
///
///   * drained per chunk  → text arrives, *then* the tool call
///   * flushed at the end → tool call first, all the text dumped after
///
/// The second one is wrong: a model that says "let me look at the file"
/// and then calls `read` would show the `▸ read` header first and the
/// sentence afterwards, which reads as the tool having interrupted it.
/// (v0.8.4 established that a tool call wins over text *on the same
/// chunk*; this is the cross-chunk case, and it must be the other way
/// round.)
#[tokio::test]
async fn text_from_an_earlier_chunk_precedes_a_later_tool_call() {
    let events = events_from_sse(&[
        r#"{"choices":[{"delta":{"content":"Let me look at the file."},"index":0}]}"#,
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_9","function":{"name":"read","arguments":"{}"}}]},"index":0}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}]}"#,
    ])
    .await;

    let text_pos = events
        .iter()
        .position(|e| matches!(e, StreamEvent::TextDelta { .. }))
        .expect("no TextDelta at all");
    let call_pos = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ToolCallStart { .. }))
        .expect("no ToolCallStart at all");

    assert!(
        text_pos < call_pos,
        "the sentence was withheld until the stream ended, so it appears \
         after the tool call it introduces: {events:?}"
    );
}

/// Aborting mid-answer must keep everything received so far. This is
/// the user-visible consequence of the queueing, and the reason the
/// drain has to be per-chunk rather than only at `finalize`.
#[tokio::test]
async fn a_second_chunk_is_not_withheld_until_the_turn_ends() {
    let events = events_from_sse(&[
        r#"{"choices":[{"delta":{"content":"first"},"index":0}]}"#,
        r#"{"choices":[{"delta":{"content":"second"},"index":0}]}"#,
    ])
    .await;

    // Only two content chunks and no stop chunk: everything must still
    // have been emitted by the time the stream closed.
    assert_eq!(visible_text(&events), "firstsecond");
    let delta_count = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::TextDelta { .. }))
        .count();
    assert_eq!(
        delta_count, 2,
        "the two chunks were merged into one event — incremental delivery \
         is broken: {events:?}"
    );
}

/// The `finalize` path: text in the LAST content chunk, with no further
/// chunk to trigger a per-call drain. This is what makes the final
/// sentence of every answer survive.
#[tokio::test]
async fn text_in_the_final_chunk_is_not_lost_at_stream_end() {
    let events = events_from_sse(&[
        r#"{"choices":[{"delta":{"content":"earlier"},"index":0}]}"#,
        // No chunk after this one carrying content — only the stop.
        r#"{"choices":[{"delta":{"content":" AND THE LAST WORDS"},"index":0}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#,
    ])
    .await;

    assert_eq!(
        visible_text(&events),
        "earlier AND THE LAST WORDS",
        "the tail of the response was dropped at stream end"
    );
}

/// The v0.7.4 MiniMax behaviour: reasoning arrives inline in
/// `delta.content` wrapped in `<think>` tags. Both halves must survive,
/// separated into their own event kinds.
#[tokio::test]
async fn think_blocks_split_into_thinking_and_visible() {
    let events = events_from_sse(&[
        r#"{"choices":[{"delta":{"content":"<think>let me think</think>"},"index":0}]}"#,
        r#"{"choices":[{"delta":{"content":"The answer is 42."},"index":0}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#,
    ])
    .await;

    assert_eq!(thinking_text(&events), "let me think");
    assert_eq!(visible_text(&events), "The answer is 42.");
    assert!(
        !visible_text(&events).contains("think"),
        "think tags leaked into the visible text"
    );
}

/// A tag split across chunk boundaries — the reason `ThinkTagFilter`
/// exists at all.
#[tokio::test]
async fn think_tag_split_across_chunks_is_handled() {
    let events = events_from_sse(&[
        r#"{"choices":[{"delta":{"content":"before <thi"},"index":0}]}"#,
        r#"{"choices":[{"delta":{"content":"nk>reasoning</thi"},"index":0}]}"#,
        r#"{"choices":[{"delta":{"content":"nk>after"},"index":0}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#,
    ])
    .await;

    assert_eq!(thinking_text(&events), "reasoning");
    assert_eq!(visible_text(&events), "before after");
}

/// A stream that ends mid-`<think>` leaves text in the filter's
/// hold-back buffer. That text is the model's last words, not a tag.
#[tokio::test]
async fn truncated_think_block_does_not_swallow_the_tail() {
    let events = events_from_sse(&[
        r#"{"choices":[{"delta":{"content":"start "},"index":0}]}"#,
        r#"{"choices":[{"delta":{"content":"<think>never clos"},"index":0}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"length","index":0}]}"#,
    ])
    .await;

    assert_eq!(visible_text(&events), "start ");
    assert_eq!(
        thinking_text(&events),
        "never clos",
        "text held in the filter buffer at stream end was discarded"
    );
    assert!(has_stop(&events));
}

/// Tool calls must still work — the fix must not regress v0.8.4's
/// ordering (tool call wins over text on a shared chunk).
#[tokio::test]
async fn tool_calls_still_arrive() {
    let events = events_from_sse(&[
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":"{\"path\":\"a.rs\"}"}}]},"index":0}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}]}"#,
    ])
    .await;

    let starts: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolCallStart { id, .. } => Some(id),
            _ => None,
        })
        .collect();
    let stops: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolCallStop { id, .. } => Some(id),
            _ => None,
        })
        .collect();

    assert_eq!(
        starts,
        vec!["call_1"],
        "tool call start missing: {events:?}"
    );
    assert_eq!(stops, vec!["call_1"], "tool call stop missing: {events:?}");
}

/// The v0.8.4 regression must stay fixed: a chunk carrying the
/// `</think>` close AND a tool call must emit the tool call first.
#[tokio::test]
async fn tool_call_wins_over_text_on_a_shared_chunk() {
    let events = events_from_sse(&[
        r#"{"choices":[{"delta":{"content":"<think>reasoning"},"index":0}]}"#,
        // This chunk closes the think block AND starts the tool call.
        r#"{"choices":[{"delta":{"content":"</think>","tool_calls":[{"index":0,"id":"call_x","function":{"name":"bash","arguments":"{}"}}]},"index":0}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}]}"#,
    ])
    .await;

    let start_pos = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ToolCallStart { .. }));
    assert!(
        start_pos.is_some(),
        "the tool call sharing a chunk with </think> was dropped: {events:?}"
    );
    assert_eq!(thinking_text(&events), "reasoning");
}

/// Usage must survive so the status bar's token counters work.
#[tokio::test]
async fn usage_is_reported() {
    let events = events_from_sse(&[
        r#"{"choices":[{"delta":{"content":"hi"},"index":0}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":1234,"completion_tokens":567}}"#,
    ])
    .await;

    match events.last() {
        Some(StreamEvent::MessageStop { usage, .. }) => {
            assert_eq!(usage.input_tokens, 1234);
            assert_eq!(usage.output_tokens, 567);
        }
        other => panic!("expected MessageStop, got {other:?}"),
    }
}

/// An empty response must still terminate cleanly rather than hanging.
#[tokio::test]
async fn empty_response_still_stops() {
    let events =
        events_from_sse(&[r#"{"choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#]).await;
    assert!(
        has_stop(&events),
        "empty response must still emit MessageStop"
    );
}

/// An error payload from the server must surface as an event, not vanish.
#[tokio::test]
async fn server_error_payload_is_surfaced() {
    let events = events_from_sse(&[
        r#"{"error":{"message":"model overloaded","type":"server_error"},"choices":[]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#,
    ])
    .await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::Error { message } if message.contains("overloaded"))),
        "the error payload was swallowed: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// 9. `delta.reasoning_content` (v0.9.0)
// ---------------------------------------------------------------------------
//
// MiniMax's reasoning models disagree about where the thinking goes, and
// the disagreement is *intermittent on the same model and prompt*:
// `MiniMax-M3` inlines it in `delta.content` inside `<think>` tags,
// while `MiniMax-M3.1-Flash-Preview` — the new default model — sends it
// as `delta.reasoning_content`.
//
// serde ignores unknown fields, so before this was handled the reasoning
// parsed fine and was silently dropped: the user saw an answer with no
// thinking block at all. Same failure shape as the v0.8.9 P0-4 text loss,
// one layer down, so it got its own coverage.

#[tokio::test]
async fn reasoning_content_becomes_a_thinking_delta() {
    let events = events_from_sse(&[
        r#"{"choices":[{"delta":{"reasoning_content":"let me think about this"},"index":0}]}"#,
        r#"{"choices":[{"delta":{"content":"The answer."},"index":0}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#,
    ])
    .await;

    assert_eq!(
        thinking_text(&events),
        "let me think about this",
        "reasoning_content was dropped: {events:?}"
    );
    assert_eq!(visible_text(&events), "The answer.");
    assert!(has_stop(&events));
}

/// A chunk carrying reasoning *and* content must emit both, reasoning
/// first. This is the v0.8.4 lesson applied to the new field: returning
/// on the first text kind drops the other.
#[tokio::test]
async fn reasoning_and_content_on_one_chunk_are_both_kept() {
    let events = events_from_sse(&[
        r#"{"choices":[{"delta":{"reasoning_content":"thinking","content":"answering"},"index":0}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#,
    ])
    .await;

    assert_eq!(
        thinking_text(&events),
        "thinking",
        "lost the reasoning: {events:?}"
    );
    assert_eq!(
        visible_text(&events),
        "answering",
        "lost the content: {events:?}"
    );

    let pos_think = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ThinkingDelta { .. }))
        .expect("no ThinkingDelta");
    let pos_text = events
        .iter()
        .position(|e| matches!(e, StreamEvent::TextDelta { .. }))
        .expect("no TextDelta");
    assert!(
        pos_think < pos_text,
        "reasoning must precede the answer: {events:?}"
    );
}

/// Reasoning interleaved with content across many chunks keeps its
/// arrival order.
#[tokio::test]
async fn interleaved_reasoning_keeps_order() {
    let events = events_from_sse(&[
        r#"{"choices":[{"delta":{"reasoning_content":"one "},"index":0}]}"#,
        r#"{"choices":[{"delta":{"content":"A "},"index":0}]}"#,
        r#"{"choices":[{"delta":{"reasoning_content":"two "},"index":0}]}"#,
        r#"{"choices":[{"delta":{"content":"B"},"index":0}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#,
    ])
    .await;

    assert_eq!(thinking_text(&events), "one two ");
    assert_eq!(visible_text(&events), "A B");
}

/// A `MiniMax-M3`-style stream that still inlines `<think>` in `content`
/// must keep working — the new field is additive, not a replacement.
#[tokio::test]
async fn inline_think_tags_still_work_alongside_the_new_field() {
    let events = events_from_sse(&[
        r#"{"choices":[{"delta":{"content":"<think>inline reasoning</think>"},"index":0}]}"#,
        r#"{"choices":[{"delta":{"content":"the answer"},"index":0}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#,
    ])
    .await;

    assert_eq!(thinking_text(&events), "inline reasoning");
    assert_eq!(visible_text(&events), "the answer");
}
