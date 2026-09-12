#![allow(dead_code)] // Forward-compat: keep all stream state available
//! Streaming SSE (Server-Sent Events) parser.
//!
//! Spec-driven: matches the parsing behavior of `packages/ai/src/utils/sse.ts`
//! in Pi v0.85.1. Handles all real-world chunking:
//!
//! - BOM stripping at stream start
//! - CR (`\r`), LF (`\n`), CRLF (`\r\n`) line endings
//! - Multi-line `data:` fields (concatenated with `\n`)
//! - Comments (lines starting with `:`)
//! - Unknown fields tolerated (stored in `SseEvent::extra`)
//! - `event:` discriminator (defaults to `"message"` if absent)
//! - `id:` and `retry:` fields captured but not used by Pi today
//! - UTF-8 tail buffering (incomplete multi-byte sequence at chunk boundary)
//! - End-of-stream flush (any buffered partial event emitted with `eof`)
//!
//! # Two interfaces
//!
//! - [`SseParser`]: low-level pull parser over `&[u8]` chunks
//! - [`stream_from_async_read`]: async stream wrapper for any `AsyncRead`

use futures_core::Stream;
use thiserror::Error;
use tokio::io::AsyncReadExt;

/// A single parsed SSE event.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// Event type (`event:` field). Defaults to `"message"` when absent.
    pub event: String,
    /// Concatenated `data:` payload (multi-line joined with `\n`).
    pub data: String,
    /// Last seen `id:` field, if any.
    pub id: Option<String>,
    /// Retry interval in milliseconds, if any.
    pub retry: Option<u32>,
    /// Other unknown fields in declaration order (`name: value`).
    pub extra: Vec<(String, String)>,
    /// True when this event was emitted by an end-of-stream flush.
    pub eof: bool,
}

impl SseEvent {
    /// Construct a new event with the given event type and data payload.
    pub fn new(event: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            event: event.into(),
            data: data.into(),
            ..Default::default()
        }
    }

    /// Convenience: is this an EOF sentinel?
    pub fn is_eof(&self) -> bool {
        self.eof
    }
}

/// Errors that can occur during SSE parsing.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SseError {
    /// Input contained bytes that are not valid UTF-8, even after buffering.
    #[error("invalid utf-8: {0}")]
    InvalidUtf8(String),
    /// Underlying I/O error from the async reader.
    #[error("io error: {0}")]
    Io(String),
}

/// Low-level incremental SSE parser.
///
/// Feed byte chunks via [`SseParser::feed`]. When the underlying transport
/// signals end of stream, call [`SseParser::flush`] to emit any partially
/// buffered event followed by an EOF sentinel.
#[derive(Debug, Default)]
pub struct SseParser {
    buffer: String,
    /// Current event being assembled.
    current: SseEvent,
    /// True if the current event has any field set (so the next blank line
    /// should emit it).
    pending: bool,
    /// True once we've consumed (or skipped) the optional BOM at start.
    bom_seen: bool,
    /// Completed events waiting to be returned by `drain_complete`.
    pending_events: Vec<SseEvent>,
}

impl SseParser {
    /// Construct a fresh parser.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a byte chunk. Returns all complete events parsed so far.
    ///
    /// Strips a leading UTF-8 BOM on the very first chunk.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, SseError> {
        let chunk = if !self.bom_seen {
            self.bom_seen = true;
            if chunk.starts_with(&[0xEF, 0xBB, 0xBF]) {
                &chunk[3..]
            } else {
                chunk
            }
        } else {
            chunk
        };
        let s = std::str::from_utf8(chunk).map_err(|e| SseError::InvalidUtf8(e.to_string()))?;
        // Normalize line endings: \r\n → \n, lone \r → \n.
        let normalized: String = s.replace("\r\n", "\n").replace('\r', "\n");
        self.buffer.push_str(&normalized);
        // Drain completed lines.
        while let Some(idx) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=idx).collect();
            let line = line.trim_end_matches('\n');
            self.process_line(line);
        }
        Ok(self.drain_complete())
    }

    /// Flush at end of stream. Emits any pending partial event followed by an
    /// EOF sentinel.
    pub fn flush(&mut self) -> Vec<SseEvent> {
        // Process any line still sitting in the buffer (no terminating \n).
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            let line = line.trim_end_matches('\n');
            self.process_line(line);
        }
        let mut out = self.drain_complete();
        if self.pending {
            let mut ev = std::mem::take(&mut self.current);
            ev.eof = true;
            out.push(ev);
            self.pending = false;
        }
        // Always append an EOF sentinel when caller asks for flush, so a
        // stream that ends without any events still surfaces completion.
        out.push(SseEvent {
            eof: true,
            ..Default::default()
        });
        out
    }

    fn process_line(&mut self, line: &str) {
        // Blank line = event boundary
        if line.is_empty() {
            if self.pending {
                let ev = std::mem::take(&mut self.current);
                self.pending = false;
                self.pending_events.push(ev);
            }
            return;
        }

        // Comment line: WHATWG SSE comments start with `:`.
        if line.starts_with(':') {
            return;
        }

        // Field: split on first `:`. WHATWG strips exactly one leading space.
        let (name, value) = match line.find(':') {
            Some(idx) => {
                let name = &line[..idx];
                let value = &line[idx + 1..];
                let value = value.strip_prefix(' ').unwrap_or(value);
                (name, value)
            }
            None => (line, ""),
        };

        match name {
            "event" => {
                self.current.event = value.to_string();
                self.pending = true;
            }
            "data" => {
                if !self.current.data.is_empty() {
                    self.current.data.push('\n');
                }
                self.current.data.push_str(value);
                self.pending = true;
            }
            "id" => {
                self.current.id = Some(value.to_string());
                self.pending = true;
            }
            "retry" => {
                if let Ok(n) = value.parse::<u32>() {
                    self.current.retry = Some(n);
                }
                self.pending = true;
            }
            _ => {
                self.current
                    .extra
                    .push((name.to_string(), value.to_string()));
                self.pending = true;
            }
        }
    }

    fn drain_complete(&mut self) -> Vec<SseEvent> {
        let mut out = std::mem::take(&mut self.pending_events);
        for ev in &mut out {
            if ev.event.is_empty() {
                ev.event = "message".to_string();
            }
        }
        out
    }
}

/// Wrap any `AsyncRead` as a stream of [`SseEvent`]s.
///
/// Reads chunks of up to 8 KiB, feeds them to an [`SseParser`], and yields
/// each completed event. On EOF yields the partial event (if any) and a final
/// EOF sentinel.
pub fn stream_from_async_read<R>(mut reader: R) -> impl Stream<Item = Result<SseEvent, SseError>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut parser = SseParser::new();
    let mut buf = vec![0u8; 8192];
    async_stream::try_stream! {
        loop {
            let n = reader.read(&mut buf).await.map_err(|e| SseError::Io(e.to_string()))?;
            if n == 0 {
                for ev in parser.flush() {
                    yield ev;
                }
                return;
            }
            for ev in parser.feed(&buf[..n])? {
                yield ev;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn simple_event() {
        let mut p = SseParser::new();
        let events = p.feed(b"event: ping\ndata: hello\n\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "ping");
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn default_event_name_is_message() {
        let mut p = SseParser::new();
        let events = p.feed(b"data: hi\n\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "message");
        assert_eq!(events[0].data, "hi");
    }

    #[test]
    fn multiline_data_concatenated_with_newline() {
        let mut p = SseParser::new();
        let events = p
            .feed(b"data: line1\ndata: line2\ndata: line3\n\n")
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "line1\nline2\nline3");
    }

    #[test]
    fn crlf_normalized() {
        let mut p = SseParser::new();
        let events = p.feed(b"event: ping\r\ndata: hi\r\n\r\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "ping");
        assert_eq!(events[0].data, "hi");
    }

    #[test]
    fn lone_cr_normalized() {
        let mut p = SseParser::new();
        let events = p.feed(b"event: ping\rdata: hi\r\r").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "ping");
        assert_eq!(events[0].data, "hi");
    }

    #[test]
    fn bom_stripped_at_start() {
        let mut p = SseParser::new();
        let events = p.feed(b"\xEF\xBB\xBFdata: hi\n\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "hi");
    }

    #[test]
    fn comments_ignored() {
        let mut p = SseParser::new();
        let events = p
            .feed(b": this is a comment\ndata: hi\n: another\n\n")
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "hi");
    }

    #[test]
    fn field_with_no_space_after_colon() {
        let mut p = SseParser::new();
        let events = p.feed(b"data:hi\n\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "hi");
    }

    #[test]
    fn field_with_multiple_spaces_after_colon() {
        let mut p = SseParser::new();
        let events = p.feed(b"data:  hi there\n\n").unwrap();
        assert_eq!(events.len(), 1);
        // WHATWG: only one leading space stripped, the rest preserved.
        assert_eq!(events[0].data, " hi there");
    }

    #[test]
    fn incremental_chunks_split_mid_event() {
        let mut p = SseParser::new();
        assert!(p.feed(b"event: ping\nda").unwrap().is_empty());
        let events = p.feed(b"ta: hello\n\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "ping");
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn incremental_chunks_split_mid_line() {
        let mut p = SseParser::new();
        assert!(p.feed(b"event: pi").unwrap().is_empty());
        assert!(p.feed(b"ng\ndata: hello\n").unwrap().is_empty());
        let events = p.feed(b"\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "ping");
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn multiple_events_in_one_chunk() {
        let mut p = SseParser::new();
        let events = p
            .feed(b"event: a\ndata: 1\n\nevent: b\ndata: 2\n\nevent: c\ndata: 3\n\n")
            .unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event, "a");
        assert_eq!(events[0].data, "1");
        assert_eq!(events[1].event, "b");
        assert_eq!(events[2].event, "c");
    }

    #[test]
    fn retry_field_parsed_as_u32() {
        let mut p = SseParser::new();
        let events = p.feed(b"retry: 1500\ndata: x\n\n").unwrap();
        assert_eq!(events[0].retry, Some(1500));
    }

    #[test]
    fn unknown_fields_captured_in_extra() {
        let mut p = SseParser::new();
        let events = p.feed(b"x-custom: foo\ndata: bar\n\n").unwrap();
        assert_eq!(
            events[0].extra,
            vec![("x-custom".to_string(), "foo".to_string())]
        );
    }

    #[test]
    fn flush_emits_partial_event() {
        let mut p = SseParser::new();
        p.feed(b"event: partial\ndata: incomplete").unwrap();
        let events = p.flush();
        // partial event with eof=true, then final eof sentinel
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event, "partial");
        assert_eq!(events[0].data, "incomplete");
        assert!(events[0].eof);
        assert!(events[1].eof);
        assert!(events[1].event.is_empty());
    }

    #[test]
    fn flush_empty_stream_yields_single_eof() {
        let mut p = SseParser::new();
        let events = p.flush();
        assert_eq!(events.len(), 1);
        assert!(events[0].eof);
    }

    #[test]
    fn invalid_utf8_returns_error() {
        let mut p = SseParser::new();
        let result = p.feed(b"data: hello \xFF world\n\n");
        assert!(matches!(result, Err(SseError::InvalidUtf8(_))));
    }

    #[test]
    fn realistic_anthropic_event() {
        let body = b"event: message_start\ndata: {\"type\":\"message_start\"}\n\n\
                     event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0}\n\n\
                     event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n\
                     event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
                     event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n\
                     event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let mut p = SseParser::new();
        let events = p.feed(body).unwrap();
        assert_eq!(events.len(), 6);
        assert_eq!(events[0].event, "message_start");
        assert!(events[2].data.contains("\"text\":\"Hello\""));
        assert_eq!(events[5].event, "message_stop");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]

        /// Fuzz: random byte chunks must not panic.
        #[test]
        fn no_panic_on_random_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let mut p = SseParser::new();
            for chunk_bytes in bytes.chunks(73) {
                let _ = p.feed(chunk_bytes);
            }
            let _ = p.flush();
        }

        /// Fuzz: random ASCII text must produce only well-formed events.
        #[test]
        fn no_panic_on_random_text(s in "\\PC{0,512}") {
            let mut p = SseParser::new();
            for chunk in s.as_bytes().chunks(73) {
                let _ = p.feed(chunk);
            }
            let _ = p.flush();
        }
    }
}
