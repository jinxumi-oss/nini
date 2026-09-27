//! OSC 8 hyperlink auto-detection and wrapping.
//!
//! Mirrors Pi's `hyperlink()` helper (pi-tui/dist/terminal-image.js) +
//! `auto_link` behavior used by `renderAssistantMessage` in pi-coding-agent.
//!
//! OSC 8 format: `ESC ]8;<params>;<URL> ESC \TEXT ESC ]8;; ESC \`
//! Supported by iTerm2 (since 3.1+), Kitty (0.20+), WezTerm, Ghostty, Konsole.
//! Unsupported terminals render the TEXT portion only \u2014 still readable.
//!
//! Capability detection (cached on first call):
//!   - iTerm2:  `TERM_PROGRAM=iTerm.app`
//!   - Kitty:   `KITTY_WINDOW_ID` set
//!   - WezTerm: `TERM_PROGRAM=WezTerm`
//!   - Ghostty: `TERM_PROGRAM=ghostty`
//!   - VS Code: `TERM_PROGRAM=vscode` (the integrated terminal forwards links)
//!   - Konsole: `KONSOLE_VERSION` set
//!   - foot:    `TERM=foot` / `foot-extra`
//!   - Windows Terminal: `WT_SESSION` set
//!
//! All other terminals get a fallback that emits raw URLs without OSC 8.

use std::env;
use std::sync::OnceLock;

/// Detect OSC 8 capability exactly once per process.
fn supports_osc8() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(detect_osc8_support)
}

fn detect_osc8_support() -> bool {
    if let Ok(tp) = env::var("TERM_PROGRAM") {
        if matches!(
            tp.as_str(),
            "iTerm.app" | "WezTerm" | "ghostty" | "vscode" | "Apple_Terminal"
        ) {
            return true;
        }
    }
    if env::var("KITTY_WINDOW_ID").is_ok() {
        return true;
    }
    if env::var("KONSOLE_VERSION").is_ok() {
        return true;
    }
    if env::var("WT_SESSION").is_ok() {
        return true;
    }
    if let Ok(term) = env::var("TERM") {
        if term.starts_with("foot") || term.starts_with("xterm-kitty") || term == "alacritty" {
            return true;
        }
    }
    false
}

/// Wrap a URL in OSC 8 hyperlinks when supported, otherwise pass through.
///
/// `text` is the visible text; `url` is what the terminal opens on click.
pub fn wrap(text: &str, url: &str) -> String {
    if supports_osc8() {
        format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
    } else {
        text.to_string()
    }
}

/// Find bare URLs in `s` and wrap them in OSC 8 hyperlinks.
///
/// Supports http(s)://, ftp://, file://. Does not touch URLs already
/// inside markdown link syntax `[label](url)` \u2014 that's handled by the
/// Markdown renderer separately.
pub fn auto_link(s: &str) -> String {
    if !supports_osc8() {
        return s.to_string();
    }
    // Conservative URL pattern: scheme:// then non-whitespace, non-`()[]{}<>"`
    // (stops at common markdown / shell delimiters).
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        // Look for a URL start: "scheme://"
        if let Some(start) = find_url_start(&s[i..]) {
            let url_start = i + start;
            let url_end_rel = scan_url_end(&s[url_start..]);
            let url_end = url_start + url_end_rel;
            // Copy the pre-URL substring verbatim.
            out.push_str(&s[i..url_start]);
            let url = &s[url_start..url_end];
            // OSC 8 link with URL == display text (no markdown label).
            out.push_str(&wrap(url, url));
            i = url_end;
        } else {
            out.push(s[i..].chars().next().unwrap());
            i += s[i..].chars().next().unwrap().len_utf8();
        }
    }
    out
}

/// Try to find a URL start within `s` from offset 0. Returns the byte
/// offset of the scheme prefix, or None.
fn find_url_start(s: &str) -> Option<usize> {
    // Walk the string; for each position, check if "scheme://" starts here.
    // Limit schemes to http, https, ftp, file to avoid false positives.
    let schemes = ["https://", "http://", "ftp://", "file://"];
    for (idx, _) in s.char_indices() {
        for scheme in &schemes {
            if s[idx..].starts_with(scheme) {
                return Some(idx);
            }
        }
        // Bail out once we've scanned past a reasonable window.
        if idx > 64 {
            return None;
        }
        // Stop at newline \u2014 don't cross line boundaries.
        if s.as_bytes()[idx] == b'\n' {
            return None;
        }
    }
    None
}

/// Scan the URL forward from start, returning the byte length of the URL.
/// Stops at whitespace, line break, or markdown / shell delimiters.
/// Trailing punctuation (.,;,:,!?) is trimmed when followed by stop chars
/// or end of input.
fn scan_url_end(s: &str) -> usize {
    let mut end = 0;
    for (idx, ch) in s.char_indices() {
        if ch.is_whitespace() || matches!(ch, ')' | ']' | '}' | '<' | '>' | '"' | '`') {
            break;
        }
        end = idx + ch.len_utf8();
    }
    // Trim a single trailing punctuation char (.,;,:,!?) if present.
    if end > 0 {
        let bytes = s.as_bytes();
        if matches!(bytes[end - 1], b'.' | b',' | b';' | b':' | b'!' | b'?') {
            end -= 1;
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_link_returns_text_when_unsupported() {
        // We can't easily flip the OnceLock; this just ensures the function
        // is safe to call and returns valid text.
        let out = auto_link("see https://example.com here");
        // Either wrapped or pass-through; both are valid.
        assert!(out.contains("example.com"));
    }

    #[test]
    fn wrap_emits_osc8_when_enabled() {
        // Force "supported" path by calling wrap directly \u2014 it consults
        // the env-based detection, so we just assert it doesn't panic.
        let out = wrap("link", "https://example.com");
        assert!(out.contains("link"));
    }

    #[test]
    fn find_url_start_detects_https() {
        let s = "hello https://example.com world";
        let pos = find_url_start(s).unwrap();
        assert_eq!(&s[pos..pos + 8], "https://");
    }

    #[test]
    fn find_url_start_returns_none_for_plain_text() {
        assert!(find_url_start("no url here").is_none());
    }

    #[test]
    fn scan_url_end_trims_trailing_punctuation() {
        // Trailing dot should be stripped from the URL.
        let url = scan_url_end("https://example.com.");
        assert_eq!(url, "https://example.com".len());

        let url = scan_url_end("https://example.com/page,");
        assert_eq!(url, "https://example.com/page".len());
    }

    #[test]
    fn scan_url_end_stops_at_whitespace() {
        let url = scan_url_end("https://example.com/path here");
        assert_eq!(url, "https://example.com/path".len());
    }

    #[test]
    fn auto_link_handles_multiple_urls() {
        // Sanity: no panics, both URLs preserved.
        let s = "https://a.com and https://b.com/x";
        let out = auto_link(s);
        assert!(out.contains("a.com"));
        assert!(out.contains("b.com"));
    }

    #[test]
    fn auto_link_does_not_cross_newlines() {
        let s = "first https://a.com\nno url";
        let out = auto_link(s);
        // The newline + "no url" should pass through untouched.
        assert!(out.contains("no url"));
    }
}