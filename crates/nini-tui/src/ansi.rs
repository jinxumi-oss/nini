//! ANSI strip stub.
pub fn strip_ansi(s: &str) -> String {
    // Minimal: remove common ESC sequences
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&c2) = chars.peek() {
                    chars.next();
                    if c2.is_ascii_alphabetic() || c2 == '~' { break; }
                }
            } else if chars.peek() == Some(&']') {
                chars.next();
                let mut term = false;
                while let Some(c2) = chars.next() {
                    if c2 == '\x07' { break; }
                    if c2 == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        term = true;
                        break;
                    }
                }
                let _ = term;
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// v0.8.4 (bugfix): truncate `s` to at most `max` *bytes*,
/// appending `…`. Uses `floor_char_boundary` so a CJK / emoji-heavy
/// string (where byte 2000 might fall inside a 3-byte char) does
/// not panic the entire TUI. nini's MSRV (1.85) predates
/// `str::floor_char_boundary` (1.91), so we use the local helper
/// in `crate::width`.
pub fn truncate(s: &str, max: usize, _max_lines: usize) -> (String, bool, u32, u32) {
    use crate::width::floor_char_boundary;
    if s.len() <= max {
        (s.to_string(), false, s.len() as u32, s.lines().count() as u32)
    } else {
        let end = floor_char_boundary(s, max);
        (format!("{}…", &s[..end]), true, s.len() as u32, s.lines().count() as u32)
    }
}
