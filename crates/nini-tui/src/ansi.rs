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

pub fn truncate(s: &str, max: usize, _max_lines: usize) -> (String, bool, u32, u32) {
    if s.len() <= max { (s.to_string(), false, s.len() as u32, s.lines().count() as u32) } else { (format!("{}…", &s[..max]), true, s.len() as u32, s.lines().count() as u32) }
}
