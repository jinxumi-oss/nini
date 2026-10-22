//! A stream must not die because a character straddles two chunks.
//!
//! # What was broken
//!
//! `SseParser::feed` decoded **each network chunk in isolation**:
//!
//! ```text
//! let s = std::str::from_utf8(chunk).map_err(|e| SseError::InvalidUtf8(e.to_string()))?;
//! ```
//!
//! A chunk boundary is wherever the network put it. A Chinese character
//! is three bytes, so one of every three of them lands on a boundary
//! from time to time — and the chunk that holds the first byte of a
//! character is not valid UTF-8 on its own. The decoder said so, and the
//! error travelled all the way up: `ProviderError::Sse`, and the turn
//! died mid-answer.
//!
//! That is a *streaming* failure, so it shows up exactly when a task is
//! running and the model is writing Chinese — the same moment the
//! report says the screen goes wrong.
//!
//! # What the invariant is
//!
//! A byte sequence that is a prefix of valid UTF-8 is not an error, it
//! is a character that has not finished arriving. The parser must hold
//! those bytes and prepend them to the next chunk.
//!
//! The assertions below are about the *stream*, not about one call: the
//! text has to come out whole, in order, no matter where the boundaries
//! fall.

use nini_ai::sse::{SseEvent, SseParser};

/// An OpenAI-style `data:` payload carrying `text`.
fn payload(text: &str) -> String {
    let mut s = String::from("data: {\"t\":\"");
    s.push_str(text);
    s.push_str("\"}\n\n");
    s
}

/// The text of every event the parser produced.
///
/// The `data:` prefix is not part of the event's data, and the end of
/// the stream adds an empty sentinel — neither is text.
fn texts(events: &[SseEvent]) -> Vec<String> {
    events
        .iter()
        .map(|e| e.data.clone())
        .filter(|d| !d.is_empty())
        .collect()
}

/// The event data of `text`, without the `data:` prefix.
fn event_data(text: &str) -> String {
    let p = payload(text);
    p.trim_end().trim_start_matches("data: ").to_string()
}

#[test]
fn a_character_split_across_two_chunks_is_not_an_error() {
    // 中 is E4 B8 AD. The first chunk holds two of the three bytes.
    let mut p = SseParser::new();
    let full = payload("\u{4e2d}");
    let bytes = full.as_bytes();
    let cut = bytes.len() - 1; // the last byte is the third byte of 中

    let events = p
        .feed(&bytes[..cut])
        .expect("a chunk that ends inside a character is not an error");
    assert!(
        texts(&events).is_empty(),
        "nothing is complete yet: {:?}",
        texts(&events)
    );

    let events = p
        .feed(&bytes[cut..])
        .expect("the rest of the character completes the event");
    assert_eq!(
        texts(&events),
        vec![event_data("\u{4e2d}")],
        "the character must survive the split"
    );
}

#[test]
fn a_character_split_across_three_chunks_is_not_an_error() {
    // The same character, one byte at a time.
    let mut p = SseParser::new();
    let full = payload("\u{4e2d}\u{6587}");
    let bytes = full.as_bytes();
    let mut got: Vec<String> = Vec::new();
    for i in 0..bytes.len() {
        let events = p
            .feed(&bytes[i..i + 1])
            .unwrap_or_else(|e| panic!("byte {i} of a Chinese string is not an error: {e}"));
        got.extend(texts(&events));
    }
    assert_eq!(got, vec![event_data("\u{4e2d}\u{6587}")]);
}

#[test]
fn a_long_chinese_answer_survives_any_chunking() {
    // The real case: the model writes Chinese, the network cuts wherever
    // it likes. Every cut must produce the same text.
    let answer = "统计结果：crates 下共有二百一十三个 Rust 源文件，其中一百零六个在 nini-tui。";
    let full = payload(answer);
    let bytes = full.as_bytes();

    for chunk_size in [1usize, 2, 3, 5, 7, 13, 64, 1024] {
        let mut p = SseParser::new();
        let mut got: Vec<String> = Vec::new();
        for chunk in bytes.chunks(chunk_size) {
            let events = p
                .feed(chunk)
                .unwrap_or_else(|e| panic!("chunk size {chunk_size} is not an error: {e}"));
            got.extend(texts(&events));
        }
        got.extend(texts(&p.flush()));
        assert_eq!(
            got,
            vec![event_data(answer)],
            "chunk size {chunk_size} changed the text"
        );
    }
}

#[test]
fn genuinely_invalid_bytes_are_still_an_error() {
    // A lone 0xFF can never be part of a character. Buffering it would
    // hide a broken stream, so it must still be reported.
    let mut p = SseParser::new();
    let err = p
        .feed(b"data: \xff\n\n")
        .expect_err("a byte that can never be valid is an error");
    assert!(
        err.to_string().contains("invalid utf-8"),
        "the error must say what is wrong: {err}"
    );
}
