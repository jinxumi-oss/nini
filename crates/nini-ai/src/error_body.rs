//! Shared helper: read a failed HTTP response's body and turn it into a
//! short, human-readable error message.
//!
//! It also builds the HTTP client the providers use ([`http_client`]),
//! because the three bounds that belong together live in one place: a
//! **bounded connect**, a **bounded error-body read**, and — just as
//! important — **no bound at all on a streaming body**.
//!
//! ## Why this module exists
//!
//! Every provider used to answer a non-2xx response with
//! `format!("request failed: {}", status)` and **threw the response body
//! away**. That produced the useless
//!
//! ```text
//! api error (400): request failed: 400 Bad Request
//!
//! ```
//!
//! while the reason — `prompt is too long`, `model not found`,
//! `invalid api key` — sat unread in the body.
//!
//! Two things break when the body is dropped:
//!
//! 1. **The user cannot diagnose the failure.** A 400 from a gateway can
//!    be a bad key, an unknown model, a request that exceeds the context
//!    window, or a malformed body; the status code alone cannot tell them
//!    apart.
//! 2. **`nini_core::overflow::is_context_overflow()` cannot fire.** It
//!    recognises an overflow by *matching the error text* against
//!    `OVERFLOW_PATTERNS` (`"prompt is too long"`, `"context_length"`,
//!    …). With the body discarded the text never reaches the detector,
//!    so a context overflow is indistinguishable from any other 400 and
//!    the long-task compaction/retry path never runs.
//!
//! ## What this module guarantees
//!
//! * **Never panics.** A body that cannot be read (a connection cut
//!   mid-response) degrades to `None`, and the caller keeps the old
//!   message.
//! * **Never hangs.** The read is bounded by
//!   [`ERROR_BODY_READ_TIMEOUT`] (3 s). A server that answers `400` and
//!   then keeps the body open forever must not be able to drag the
//!   request down with it — the error was known the moment the status
//!   line arrived. On the deadline the body is dropped and the caller
//!   falls back to the plain `request failed: <status>` message.
//! * **Never dumps a whole page.** An HTML error page is stripped to its
//!   text and capped hard; the result is capped at
//!   [`MAX_MESSAGE_BYTES`] in every case.
//! * **Prefers the structured message.** The server's own wording is
//!   reproduced verbatim whenever it is available, because that wording
//!   is what both humans and the overflow detector match on.
//! * **Keeps the provider's error code.** A provider's numeric code is
//!   the only thing that tells two failures with the *same* message
//!   apart: iFlytek's `10907` ("context too long") and `10005`
//!   ("invalid request") both arrive as `400 Bad Request`, and the
//!   message alone ("Bad Request") cannot separate them. The code is
//!   appended as `(code 10907)`; the message stays verbatim.
//!
//! ## What this layer does **not** do
//!
//! It does not explain what to *do* about a failure, and it does not
//! guess *why* the server refused the request. The provider layer
//! knows one thing — the HTTP status, the server's own words and the
//! vendor's error code — and it says exactly that:
//!
//! ```text
//! request failed: 400 Bad Request: Bad Request (code 10907)
//! ```
//!
//! A hint that named causes ("an empty user message, a wrong model
//! id, …") was both a guess and a duplicate: the agent layer
//! (`nini_core::agent::annotate_request_rejection`) already appends
//! the *actionable* advice (`/compact`, shrink the task, switch to a
//! model with a bigger window) once it has classified the failure —
//! and only it knows how long the conversation is. Two hints stacked
//! up into one long, self-contradicting sentence that repeated
//! "the server rejected the request" twice and, worse, blamed causes
//! that were not the real one (the 400 in production was a context
//! overflow, iFlytek `10907`).
//!
//! So the split is:
//!
//! * **provider layer (here)** — facts only: status, verbatim server
//!   text, vendor error code.
//! * **agent layer (`nini_core`)** — advice, because only it knows the
//!   session state the advice depends on.

use serde_json::Value;
use std::time::Duration;

/// Hard cap on the extracted message, in bytes.
///
/// An error message is something a human reads in a terminal and a
/// pattern matcher searches. 4 KiB is far more than any real API error
/// needs and far less than a gateway's HTML page.
pub const MAX_MESSAGE_BYTES: usize = 4096;

/// How much of the raw body is read before giving up.
///
/// Reading is bounded so a hostile or broken server cannot make nini
/// buffer an unbounded response. It is larger than
/// [`MAX_MESSAGE_BYTES`] so that a JSON error body is normally read in
/// one piece and can still be parsed as JSON.
const MAX_READ_BYTES: usize = 8192;

/// How long the **error body** may take to arrive before it is
/// abandoned.
///
/// ## Why a timeout is mandatory here
///
/// Reading a failed response's body is a courtesy, not the work. The
/// work — the request — already failed. But a server that answers
/// `400` and then keeps the body open (a chunked response that never
/// terminates, a proxy that streams the error page lazily) would make
/// nini wait for a body that is never coming: the error the user needs
/// is *already known*, and the request hangs anyway.
///
/// That is strictly worse than the behaviour before this module existed
/// (the body was simply not read, and the error came back at once), so
/// the read has to be bounded.
///
/// ## Why 3 seconds
///
/// * The useful part of an error body — the JSON, or the first sentence
///   of the text — arrives in the first packets. Anything that has not
///   arrived in three seconds over a connection that is already up is
///   not going to be worth the user's wait.
/// * It is long enough for a slow gateway (a mobile link, a TLS
///   handshake that was already done, a busy proxy) to deliver a few
///   kilobytes, and short enough that the user is not staring at a
///   spinner wondering whether nini crashed.
/// * It is *only* the body. The request itself, and the streaming
///   response that follows a successful one, are never bounded by it —
///   see [`http_client`].
///
/// When the deadline passes, the body is dropped and the caller falls
/// back to the plain `request failed: <status>` message. A timeout is
/// a degradation, never a hang and never a panic.
pub const ERROR_BODY_READ_TIMEOUT: Duration = Duration::from_secs(3);

/// How long establishing the connection (TCP + TLS) may take.
///
/// The one bound that is safe on *every* request, streaming or not:
/// a server that cannot be reached must not hang nini forever.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Build the HTTP client every provider uses.
///
/// ## What is bounded, and what is deliberately not
///
/// * **Connect: [`CONNECT_TIMEOUT`].** A dead host must not hang the
///   request.
/// * **The response body of a *failed* request: [`ERROR_BODY_READ_TIMEOUT`].**
///   A failed request's body is a small piece of text, not a stream.
/// * **The response body of a *successful* request: no timeout.** A
///   streaming response (SSE) is *supposed* to stay open for minutes —
///   a thinking model can think for a while before the first token. A
///   total timeout would kill a perfectly healthy stream, so reqwest's
///   client-wide `timeout` is deliberately **not** set here.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        // A client that cannot be built is a configuration error, but it
        // must not take the process down: fall back to the defaults.
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Cap for a body that turned out to be an HTML page.
///
/// An HTML page is not an error message; it is a page. Only its text is
/// kept, and only a little of it.
const MAX_HTML_BYTES: usize = 512;

/// Marker appended when the message had to be cut short.
const TRUNCATION_MARKER: &str = "… [truncated]";

/// A failed request, as the server described it.
///
/// The `message` is the server's own wording, verbatim. The `code` is
/// the provider's error code, kept as text — it is a number on some
/// providers (`10907`) and a slug on others (`context_length_exceeded`),
/// and both forms are needed to tell two failures apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorDetail {
    /// The server's own wording, verbatim.
    pub message: String,
    /// The provider's error code, as text.
    pub code: Option<String>,
}

/// Hard bound on the *composed* message.
///
/// The extracted text is capped at [`MAX_MESSAGE_BYTES`]; the prefix
/// and the error code add a bounded amount on top of it.
pub const MAX_COMPOSED_MESSAGE_BYTES: usize = MAX_MESSAGE_BYTES + 512;

/// Build the final `ProviderError::Api` message for a failed request.
///
/// The prefix is unchanged from the previous behaviour so that existing
/// assertions (and the log format) stay valid; the server's own text is
/// appended when there is any, followed by the provider's error code.
///
/// ```text
/// request failed: 400 Bad Request: prompt is too long: 300000 tokens > 200000
/// request failed: 400 Bad Request: Bad Request (code 10907)
/// request failed: 400 Bad Request: (code 10907)
/// ```
///
/// A `400` is **not** given a hint here: the provider layer states the
/// facts and stops. The advice ("try `/compact`", "switch to a model
/// with a bigger window") belongs to the agent layer, which knows how
/// long the conversation actually is and already appends it on the
/// `ProviderError::Api { status, .. }` it receives.
pub fn compose_api_message(status: reqwest::StatusCode, extracted: Option<ErrorDetail>) -> String {
    let mut out = format!("request failed: {status}");
    if let Some(detail) = extracted {
        let code = detail.code.as_deref().filter(|c| !c.is_empty());
        match code {
            Some(code) if code == detail.message => {
                // The code *is* the message (`{"error": {"code": 10907}}`,
                // `{"code": 10907}`). It must appear exactly once — but
                // it has to appear **labelled**: the label is what
                // `nini_core::overflow::extract_vendor_error_code` looks
                // for, and a bare `10907` is not a code to it.
                out.push_str(": (code ");
                out.push_str(code);
                out.push(')');
            }
            Some(code) => {
                if !detail.message.is_empty() {
                    out.push_str(": ");
                    out.push_str(&detail.message);
                }
                out.push_str(&format!(" (code {code})"));
            }
            None => {
                if !detail.message.is_empty() {
                    out.push_str(": ");
                    out.push_str(&detail.message);
                }
            }
        }
    }
    // Keep the whole message bounded. The extracted text is already
    // capped; this is the belt for a caller that built an
    // `ErrorDetail` by hand.
    if out.len() > MAX_COMPOSED_MESSAGE_BYTES {
        out = cap_bytes(&out, MAX_COMPOSED_MESSAGE_BYTES);
    }
    out
}

/// Read a failed response and extract what the server said about it.
///
/// Returns `None` when the body is empty, unreadable, or carries nothing
/// a human would recognise as a message — the caller then falls back to
/// the plain `request failed: <status>` text.
///
/// Never panics: a read failure is a `None`, not a crash.
///
/// Takes the response by mutable reference so a caller can keep using it
/// (the success path still needs to stream it) — the body is only read
/// on the failure path.
///
/// ## Bounded by [`ERROR_BODY_READ_TIMEOUT`]
///
/// The read is a courtesy; it must never be able to hang the request.
/// A server that answers `400` and then keeps the body open forever
/// used to hang nini indefinitely — strictly worse than not reading the
/// body at all. On the deadline the body is dropped and the caller
/// falls back to the plain `request failed: <status>` message.
pub async fn extract_error_detail(response: &mut reqwest::Response) -> Option<ErrorDetail> {
    let read = tokio::time::timeout(
        ERROR_BODY_READ_TIMEOUT,
        read_capped(response, MAX_READ_BYTES),
    )
    .await;
    let bytes = match read {
        // The body arrived in time.
        Ok(Ok(bytes)) => bytes,
        // The connection broke mid-body: no message, no crash.
        Ok(Err(_)) => return None,
        // The deadline passed: the body is dropped, the request is not
        // dragged down with it.
        Err(_elapsed) => return None,
    };
    extract_detail_from_bytes(&bytes)
}

/// Read at most `limit` bytes of the response body.
async fn read_capped(
    response: &mut reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, reqwest::Error> {
    let mut out: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        let room = limit.saturating_sub(out.len());
        if chunk.len() >= room {
            out.extend_from_slice(&chunk[..room]);
            break;
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Extract the error message out of a raw response body.
///
/// Pure function — the wire-level tests use it directly, and it is where
/// all the shape-handling lives:
///
/// 1. OpenAI / Anthropic style:
///    `{"error": {"message": "...", "type": "...", "code": "..."}}`
/// 2. Top-level `{"message": ...}`, `{"error": "<string>"}`,
///    `{"msg": ...}`, `{"detail": ...}`
/// 3. Anything else: plain text, cleaned of control characters and
///    flattened onto one line.
pub fn extract_from_bytes(body: &[u8]) -> Option<String> {
    extract_detail_from_bytes(body).map(|detail| detail.message)
}

/// Extract the message **and** the provider's error code out of a raw
/// response body.
///
/// The code is what makes two otherwise identical failures
/// distinguishable: iFlytek answers `10907` for "context too long" and
/// `10005` for "invalid request", and both arrive as `400 Bad Request`.
pub fn extract_detail_from_bytes(body: &[u8]) -> Option<ErrorDetail> {
    // A NUL byte means this is not text we want to show (a gzipped
    // response that reqwest did not decompress, a PNG, …).
    if body.contains(&0) {
        return None;
    }

    let raw = String::from_utf8_lossy(body);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 1 + 2: a structured body.
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        if let Some(detail) = detail_from_json(&value) {
            return Some(finalize_detail(detail));
        }
        // A JSON body we could not make sense of (an array, a number):
        // fall through to the plain-text path rather than dropping it.
    }

    // 3: plain text.
    let looks_like_html = trimmed.starts_with('<')
        && (trimmed.to_ascii_lowercase().contains("<html")
            || trimmed.to_ascii_lowercase().contains("<!doctype")
            || trimmed.contains("</"));
    if looks_like_html {
        let text = strip_tags(trimmed);
        let text = sanitize(&text);
        if text.is_empty() {
            return None;
        }
        return Some(ErrorDetail {
            message: finalize(cap_bytes(&text, MAX_HTML_BYTES)),
            code: None,
        });
    }

    let text = sanitize(trimmed);
    if text.is_empty() {
        return None;
    }
    Some(ErrorDetail {
        message: finalize(cap_bytes(&text, MAX_MESSAGE_BYTES)),
        code: None,
    })
}

/// Apply the last touches every extracted message goes through:
/// credential redaction, then the length cap.
fn finalize(message: String) -> String {
    cap_bytes(&redact(&message), MAX_MESSAGE_BYTES)
}

/// The same, for a message that travels with a code: only the message
/// is redacted and capped — a code is a short token by construction.
fn finalize_detail(detail: ErrorDetail) -> ErrorDetail {
    ErrorDetail {
        message: finalize(detail.message),
        code: detail.code,
    }
}

/// The message *and* the code of a parsed JSON body.
///
/// The code is looked up independently of the message: a body may
/// carry a message without a code, a code without a message (the code
/// then *is* the message), or both.
fn detail_from_json(value: &Value) -> Option<ErrorDetail> {
    let code = code_from_json(value);
    let message = match message_from_json(value) {
        Some(message) => message,
        // No message anywhere in the body — but there *is* a code
        // (`{"code": 10907}`). The code is then the only thing the
        // server said, and it must not be lost: falling through to the
        // plain-text path would turn the whole body
        // (`{"code":10907}`) into the message, which is exactly the
        // shape that hides the code the user needs.
        None => code.clone()?,
    };
    Some(ErrorDetail { message, code })
}

/// The provider's error code, as text.
///
/// A code is a number on some providers (`10907`) and a slug on others
/// (`context_length_exceeded`); both are kept, the number rendered as
/// its JSON text.
///
/// Looked up in this order:
/// 1. `error.code` — the OpenAI / Anthropic / iFlytek shape
/// 2. `code` — some gateways put it at the top level
/// 3. `error.error.code` — a doubly nested error object
fn code_from_json(value: &Value) -> Option<String> {
    let err = value.get("error");
    if let Some(code) = err.and_then(|e| e.get("code")).and_then(scalar_text) {
        return Some(code);
    }
    if let Some(code) = value.get("code").and_then(scalar_text) {
        return Some(code);
    }
    if let Some(code) = err
        .and_then(|e| e.get("error"))
        .and_then(|e| e.get("code"))
        .and_then(scalar_text)
    {
        return Some(code);
    }
    None
}

/// A JSON string or number as text, if that is what the value is.
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        }
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Pull a human-readable message out of a parsed JSON body.
///
/// The search order is the one the wire actually uses:
///
/// * `error.message` / `error.code` / `error.type` (OpenAI, Anthropic)
/// * `error` as a plain string (some gateways)
/// * top-level `message`, `msg`, `detail`
fn message_from_json(value: &Value) -> Option<String> {
    if let Some(err) = value.get("error") {
        if let Some(text) = string_field(err, &["message", "code", "type"]) {
            return Some(text);
        }
        if let Some(text) = string_field(err, &["msg", "detail"]) {
            return Some(text);
        }
        // `{"error": "model not found"}` — the whole error *is* the text.
        if let Some(text) = as_text(err) {
            return Some(text);
        }
        // `error` may itself hold a nested `{"error": {"message": ...}}`.
        if let Some(nested) = err.get("error") {
            if let Some(text) = string_field(nested, &["message", "code", "type"]) {
                return Some(text);
            }
        }
    }

    for key in ["message", "msg", "detail"] {
        if let Some(text) = string_field(value, &[key]) {
            return Some(text);
        }
        // `detail` is an object on FastAPI-style servers.
        if let Some(nested) = value.get(key) {
            if nested.is_object() {
                if let Some(text) = string_field(nested, &["message", "msg", "error", "detail"]) {
                    return Some(text);
                }
            }
        }
    }

    // `{"errors": ["..."]}` — a list of per-field messages.
    if let Some(list) = value.get("errors").and_then(Value::as_array) {
        let parts: Vec<String> = list
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .filter(|s| !s.trim().is_empty())
            .collect();
        if !parts.is_empty() {
            return Some(parts.join("; "));
        }
    }

    None
}

/// A JSON string (or number) as text, if that is what the value is.
fn as_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        }
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The first non-empty string among `keys` of `value`.
///
/// Numbers are accepted (an error `code` is often numeric) and rendered
/// as their JSON text.
fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        let Some(field) = value.get(key) else {
            continue;
        };
        let text = match field {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => continue,
        };
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    None
}

/// Flatten text onto one line: drop control characters, collapse every
/// whitespace run into a single space, trim the ends.
fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if ch.is_control() || ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(ch);
    }
    out
}

/// Remove HTML tags (and the contents of `<script>` / `<style>` blocks).
///
/// A gateway's 502 page is a page, not a message. Keeping the tags would
/// bury the one useful sentence in markup, so the markup goes and the
/// text stays.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // Skip the whole element, including its content for the
            // elements whose content is never prose.
            let rest = &html[i..];
            let lower = rest.to_ascii_lowercase();
            let skip_content = lower.starts_with("<script")
                || lower.starts_with("<style")
                || lower.starts_with("<head");
            if skip_content {
                let tag = if lower.starts_with("<script") {
                    "</script>"
                } else if lower.starts_with("<style") {
                    "</style>"
                } else {
                    "</head>"
                };
                match lower.find(tag) {
                    Some(end) => {
                        i += end + tag.len();
                        continue;
                    }
                    None => {
                        // Unterminated block: drop the rest.
                        break;
                    }
                }
            }
            match rest.find('>') {
                Some(end) => {
                    i += end + 1;
                    continue;
                }
                None => break,
            }
        }
        let ch = html[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Cut `text` to `limit` bytes, marking it as truncated.
fn cap_bytes(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let keep = limit.saturating_sub(TRUNCATION_MARKER.len());
    let mut end = keep.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = text[..end].to_string();
    out.push_str(TRUNCATION_MARKER);
    out
}

/// Redact things that look like credentials.
///
/// An error body sometimes echoes the key that was sent
/// (`invalid key sk-abc…`). That text ends up in logs and transcripts,
/// so the obvious shapes are masked before the message is shown.
fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("sk-") {
        let (head, tail) = rest.split_at(pos);
        out.push_str(head);
        // The token runs until the next whitespace or quote.
        let end = tail
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
            .unwrap_or(tail.len());
        let token = &tail[..end];
        if token.len() >= 16 {
            out.push_str("sk-***");
            rest = &tail[end..];
        } else {
            out.push_str("sk-");
            rest = &tail["sk-".len()..];
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_openai_shaped_error_yields_its_message() {
        let body = br#"{"error":{"message":"prompt is too long","type":"invalid_request_error","code":"context_length_exceeded"}}"#;
        assert_eq!(
            extract_from_bytes(body).as_deref(),
            Some("prompt is too long")
        );
    }

    #[test]
    fn an_anthropic_shaped_error_yields_its_message() {
        let body =
            br#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(extract_from_bytes(body).as_deref(), Some("Overloaded"));
    }

    #[test]
    fn a_top_level_message_is_used() {
        let body = br#"{"message":"invalid api key"}"#;
        assert_eq!(extract_from_bytes(body).as_deref(), Some("invalid api key"));
    }

    #[test]
    fn an_error_string_is_used() {
        let body = br#"{"error":"model not found"}"#;
        assert_eq!(extract_from_bytes(body).as_deref(), Some("model not found"));
    }

    #[test]
    fn a_msg_field_is_used() {
        let body = br#"{"msg":"quota exceeded"}"#;
        assert_eq!(extract_from_bytes(body).as_deref(), Some("quota exceeded"));
    }

    #[test]
    fn a_detail_field_is_used() {
        let body = br#"{"detail":"tool_use is not supported"}"#;
        assert_eq!(
            extract_from_bytes(body).as_deref(),
            Some("tool_use is not supported")
        );
    }

    #[test]
    fn a_nested_detail_object_is_used() {
        let body = br#"{"detail":{"message":"field required","type":"value_error"}}"#;
        assert_eq!(extract_from_bytes(body).as_deref(), Some("field required"));
    }

    #[test]
    fn a_numeric_code_is_used_when_there_is_no_message() {
        let body = br#"{"error":{"code":40001}}"#;
        assert_eq!(extract_from_bytes(body).as_deref(), Some("40001"));
    }

    #[test]
    fn plain_text_is_kept_but_flattened() {
        let body = b"Internal Server Error\n\r\nplease retry";
        assert_eq!(
            extract_from_bytes(body).as_deref(),
            Some("Internal Server Error please retry")
        );
    }

    #[test]
    fn an_empty_body_yields_nothing() {
        assert_eq!(extract_from_bytes(b""), None);
        assert_eq!(extract_from_bytes(b"   \n  "), None);
    }

    #[test]
    fn a_binary_body_yields_nothing() {
        assert_eq!(extract_from_bytes(&[0x1f, 0x8b, 0x00, 0x42]), None);
    }

    #[test]
    fn an_html_page_is_reduced_to_its_text() {
        let body =
            b"<html><head><title>502</title></head><body><h1>502 Bad Gateway</h1></body></html>";
        let out = extract_from_bytes(body).expect("html body must yield text");
        assert!(out.contains("502 Bad Gateway"), "got {out:?}");
        assert!(!out.contains('<'), "tags survived: {out:?}");
        assert!(out.len() <= MAX_HTML_BYTES + TRUNCATION_MARKER.len());
    }

    #[test]
    fn a_long_message_is_truncated_and_marked() {
        let long = "x".repeat(MAX_MESSAGE_BYTES * 2);
        let out = extract_from_bytes(long.as_bytes()).expect("long body must yield text");
        assert!(out.ends_with(TRUNCATION_MARKER), "got {out:?}");
        assert!(out.len() <= MAX_MESSAGE_BYTES, "len was {}", out.len());
    }

    #[test]
    fn a_credential_is_masked() {
        let body = br#"{"error":{"message":"invalid api key sk-abcdefghijklmnopqrstuv"}}"#;
        let out = extract_from_bytes(body).expect("body must yield text");
        assert!(out.contains("sk-***"), "got {out:?}");
        assert!(!out.contains("abcdefghijklmnopqrstuv"), "got {out:?}");
    }

    #[test]
    fn the_composed_message_keeps_the_old_prefix() {
        let status = reqwest::StatusCode::BAD_REQUEST;
        assert_eq!(
            compose_api_message(status, None),
            "request failed: 400 Bad Request"
        );
        assert_eq!(
            compose_api_message(
                status,
                Some(ErrorDetail {
                    message: "prompt is too long".to_string(),
                    code: None,
                })
            ),
            "request failed: 400 Bad Request: prompt is too long"
        );
    }

    /// The provider layer states facts; the agent layer states advice.
    ///
    /// A provider-side hint used to be appended to every 400, and the
    /// agent layer appends its own — the two stacked up into one
    /// sentence that said "the server rejected the request" twice, and
    /// the provider-side half also *guessed* causes ("an empty user
    /// message, a wrong model id") that the provider cannot know and
    /// that were not the real cause (iFlytek `10907`, a context
    /// overflow).
    #[test]
    fn the_provider_layer_carries_no_hint_of_its_own() {
        for detail in [
            None,
            Some(ErrorDetail {
                message: "Bad Request".to_string(),
                code: Some("10907".to_string()),
            }),
            Some(ErrorDetail {
                message: "invalid request".to_string(),
                code: Some("10005".to_string()),
            }),
        ] {
            let composed = compose_api_message(reqwest::StatusCode::BAD_REQUEST, detail);
            assert!(
                !composed.contains("the server rejected the request"),
                "the provider layer must not add the agent layer's advice: {composed:?}"
            );
            assert!(
                !composed.contains("hint:"),
                "the provider layer must not add a hint at all: {composed:?}"
            );
            assert!(
                !composed.contains("/compact"),
                "the advice belongs to the agent layer: {composed:?}"
            );
        }
    }

    /// The facts survive the removal of the hint: status, the server's
    /// own words, the vendor's error code.
    #[test]
    fn the_facts_of_a_400_survive() {
        assert_eq!(
            compose_api_message(
                reqwest::StatusCode::BAD_REQUEST,
                Some(ErrorDetail {
                    message: "Bad Request".to_string(),
                    code: Some("10907".to_string()),
                })
            ),
            "request failed: 400 Bad Request: Bad Request (code 10907)"
        );
        assert_eq!(
            compose_api_message(
                reqwest::StatusCode::BAD_REQUEST,
                Some(ErrorDetail {
                    message: "invalid request".to_string(),
                    code: Some("10005".to_string()),
                })
            ),
            "request failed: 400 Bad Request: invalid request (code 10005)"
        );
        assert_eq!(
            compose_api_message(reqwest::StatusCode::BAD_REQUEST, None),
            "request failed: 400 Bad Request"
        );
    }

    #[test]
    fn a_numeric_code_is_kept_next_to_the_message() {
        let detail =
            extract_detail_from_bytes(br#"{"error":{"code":10907,"message":"Bad Request"}}"#)
                .expect("a structured body must yield a detail");
        assert_eq!(detail.message, "Bad Request");
        assert_eq!(detail.code.as_deref(), Some("10907"));
        assert_eq!(
            compose_api_message(reqwest::StatusCode::BAD_REQUEST, Some(detail)),
            "request failed: 400 Bad Request: Bad Request (code 10907)"
        );
    }

    #[test]
    fn a_string_code_is_kept_next_to_the_message() {
        let detail = extract_detail_from_bytes(
            br#"{"error":{"message":"prompt is too long","code":"context_length_exceeded"}}"#,
        )
        .expect("a structured body must yield a detail");
        assert_eq!(detail.message, "prompt is too long");
        assert_eq!(detail.code.as_deref(), Some("context_length_exceeded"));
    }

    #[test]
    fn a_code_without_a_message_is_not_repeated() {
        let detail =
            extract_detail_from_bytes(br#"{"error":{"code":40001}}"#).expect("a code is a message");
        assert_eq!(detail.message, "40001");
        assert_eq!(detail.code.as_deref(), Some("40001"));
        let composed = compose_api_message(reqwest::StatusCode::BAD_REQUEST, Some(detail));
        assert_eq!(
            composed.matches("40001").count(),
            1,
            "the code must appear once: {composed:?}"
        );
    }

    #[test]
    fn a_top_level_code_is_kept() {
        let detail = extract_detail_from_bytes(br#"{"code":10005,"message":"invalid request"}"#)
            .expect("a structured body must yield a detail");
        assert_eq!(detail.message, "invalid request");
        assert_eq!(detail.code.as_deref(), Some("10005"));
    }

    #[test]
    fn a_nested_error_object_keeps_its_code() {
        let detail =
            extract_detail_from_bytes(br#"{"error":{"error":{"message":"boom","code":"E42"}}}"#)
                .expect("a structured body must yield a detail");
        assert_eq!(detail.message, "boom");
        assert_eq!(detail.code.as_deref(), Some("E42"));
    }

    /// A body that carries **only** a code must not lose it.
    ///
    /// Before the fix, `message_from_json` had no fallback for the
    /// top-level `code`: the body fell through to the plain-text path
    /// and the *whole JSON* became the message —
    /// `{"code":10907}` — which is the one shape that hides the very
    /// code the user needs.
    #[test]
    fn a_top_level_code_alone_is_not_lost() {
        let detail = extract_detail_from_bytes(br#"{"code":10907}"#)
            .expect("a body with a code must yield a detail");
        assert_eq!(detail.message, "10907");
        assert_eq!(detail.code.as_deref(), Some("10907"));

        let composed = compose_api_message(reqwest::StatusCode::BAD_REQUEST, Some(detail));
        assert!(
            composed.contains("10907"),
            "the code was lost: {composed:?}"
        );
        assert!(
            !composed.contains('{') && !composed.contains('}'),
            "the raw JSON leaked into the message: {composed:?}"
        );
        assert_eq!(
            composed, "request failed: 400 Bad Request: (code 10907)",
            "the code must be labelled so the vendor-code extractor finds it"
        );
    }

    /// The same for the string form of a code.
    #[test]
    fn a_top_level_string_code_alone_is_not_lost() {
        let detail = extract_detail_from_bytes(br#"{"code":"context_length_exceeded"}"#)
            .expect("a body with a code must yield a detail");
        assert_eq!(detail.message, "context_length_exceeded");
        assert_eq!(detail.code.as_deref(), Some("context_length_exceeded"));

        let composed = compose_api_message(reqwest::StatusCode::BAD_REQUEST, Some(detail));
        assert!(
            composed.contains("context_length_exceeded"),
            "the code was lost: {composed:?}"
        );
        assert!(
            !composed.contains('{') && !composed.contains('}'),
            "the raw JSON leaked into the message: {composed:?}"
        );
        assert_eq!(
            composed,
            "request failed: 400 Bad Request: (code context_length_exceeded)"
        );
    }

    /// A body with a code and a message keeps both, and the code is
    /// labelled once.
    #[test]
    fn a_code_next_to_a_message_is_labelled_once() {
        let composed = compose_api_message(
            reqwest::StatusCode::BAD_REQUEST,
            extract_detail_from_bytes(br#"{"code":10907,"message":"Bad Request"}"#),
        );
        assert_eq!(
            composed,
            "request failed: 400 Bad Request: Bad Request (code 10907)"
        );
        assert_eq!(composed.matches("10907").count(), 1, "{composed:?}");
    }
}
