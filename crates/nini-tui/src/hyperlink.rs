//! Auto-link URLs in text via OSC 8 (passthrough on terminals that don't support it).

pub fn auto_link(s: &str) -> String {
    // Minimal implementation: leave text as-is. Real OSC 8 wrapping would go here.
    s.to_string()
}
