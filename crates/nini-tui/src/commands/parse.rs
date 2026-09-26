//! v0.8.3: Parse `/<cmd> <args>` input into `(CommandId, args)`.
//!
//! Pure function — no state mutation, no I/O.

use super::registry::{by_name, CommandId};

/// Parse `/<cmd> <args>` input. Strips whitespace, looks up the
/// command name in the registry. Returns None for non-`/` input
/// or unknown commands.
pub fn parse(input: &str) -> Option<(CommandId, String)> {
    let trimmed = input.trim();
    let rest = trimmed.strip_prefix('/')?;
    if rest.is_empty() {
        return None;
    }
    let (name, args) = match rest.find(char::is_whitespace) {
        Some(i) => (&rest[..i], rest[i + 1..].trim().to_string()),
        None => (rest, String::new()),
    };
    let def = by_name(name)?;
    Some((def.id, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_command() {
        let (id, args) = parse("/model anthropic/claude-opus-4-7").unwrap();
        assert_eq!(id, CommandId::Model);
        assert_eq!(args, "anthropic/claude-opus-4-7");
    }

    #[test]
    fn parse_no_args() {
        let (id, args) = parse("/quit").unwrap();
        assert_eq!(id, CommandId::Quit);
        assert_eq!(args, "");
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert!(parse("/notacommand").is_none());
    }

    #[test]
    fn parse_without_slash_returns_none() {
        assert!(parse("hello world").is_none());
    }

    #[test]
    fn parse_empty_returns_none() {
        assert!(parse("").is_none());
        assert!(parse("/").is_none());
        assert!(parse("   ").is_none());
    }

    #[test]
    fn parse_trims_extra_whitespace() {
        let (id, args) = parse("  /help   extra  ").unwrap();
        assert_eq!(id, CommandId::Help);
        assert_eq!(args, "extra");
    }
}