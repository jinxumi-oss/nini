//! OSC 52 — ask the **terminal** to put text on the clipboard.
//!
//! Pi uses the same channel when the process has no native clipboard
//! (`references/pi-0.85.1/src/tui/tui-alt-screen.ts` — the copy-on-select
//! path falls back to `\x1b]52;c;<base64>\x07`).
//!
//! It exists in nini because the native clipboard is not always there:
//! a headless terminal, an SSH session without a clipboard bridge, or
//! tmux without `set-clipboard` — the copy fails and the user is left
//! with nothing.
//!
//! # What it can and cannot promise
//!
//! OSC 52 is a **request to the terminal**, not a clipboard write nini can
//! verify. Whether the terminal honours it depends on the terminal and
//! its settings. So the caller must not claim the text was copied — the
//! feedback says "via OSC 52 (the terminal may not keep a clipboard)".
//! This mirrors `clipboard.rs`'s iron rule: never report a success that
//! cannot be verified.
//!
//! # Why this is its own module
//!
//! It is the **only** place in the TUI that writes to stdout while the
//! TUI owns the screen, so it is the only place the
//! `no_terminal_writes` invariant needs an entry for. The write is an
//! escape sequence, not text: it cannot corrupt the painted frame the
//! way a stray `println!` does.

/// Standard base64 alphabet (RFC 4648) with `=` padding.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode `bytes` as standard base64.
///
/// No dependency: the repository has no base64 crate, and twenty lines
/// here are cheaper than a new dependency for one escape sequence.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

// Test seam: make the OSC 52 write fail.
//
// The third feedback path — "the native clipboard failed **and** OSC 52
// failed" — is the one that must never claim a copy, and it needs a
// deterministic way to make the OSC 52 write fail.
#[cfg(test)]
thread_local! {
    static OSC52_FAILS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Ask the terminal to put `text` on its clipboard.
///
/// The write goes to stdout between frames (the mouse handler, not the
/// render pass), and it is an escape sequence, so it cannot corrupt the
/// painted frame.
pub fn write_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    #[cfg(test)]
    if OSC52_FAILS.with(|f| f.get()) {
        return Err("OSC 52 write disabled by the test seam".to_string());
    }
    let payload = base64_encode(text.as_bytes());
    // `print!` rather than a `write!` on a stdout handle: the handle
    // spelling is invisible to the `no_terminal_writes` invariant, and a
    // write that the invariant cannot see is a write nobody reviews.
    print!("\x1b]52;c;{payload}\x07");
    std::io::stdout()
        .flush()
        .map_err(|e| format!("OSC 52 flush failed: {e}"))
}

#[cfg(test)]
pub(crate) fn set_fail_for_tests(value: bool) {
    OSC52_FAILS.with(|f| f.set(value));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_handles_non_ascii_bytes() {
        // "你" is three UTF-8 bytes.
        assert_eq!(base64_encode("你".as_bytes()), "5L2g");
    }

    #[test]
    fn the_escape_sequence_carries_the_payload() {
        let payload = base64_encode(b"hello");
        assert_eq!(payload, "aGVsbG8=");
        // The sequence the terminal receives.
        assert_eq!(format!("\x1b]52;c;{payload}\x07"), "\x1b]52;c;aGVsbG8=\x07");
    }
}
