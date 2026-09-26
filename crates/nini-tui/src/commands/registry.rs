//! v0.8.3: Slash command registry — pure data, no dispatch logic.
//!
//! Holds the `CommandId` enum (28 variants), `CommandDef` struct
//! (name + description + argument hint), the static `REGISTRY`
//! array, and the lookup helpers `by_name()` + `complete()`.
//!
//! Pure data module — no `AppState` access, no dispatch logic. The
//! other sub-modules depend on this; this module depends on nothing.

/// 28 slash commands (canonical order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandId {
    Settings,
    Model,
    Tree,
    Thinking,
    Status,
    Help,
    Hotkeys,
    Debug,
    Editor,
    Login,
    Logout,
    Share,
    Changelog,
    Trust,
    New,
    Resume,
    Export,
    Import,
    Fork,
    Clone,
    Compact,
    Session,
    ScopedModels,
    Name,
    Copy,
    Reload,
    Prompt,
    Quit,
}

pub struct CommandDef {
    pub id: CommandId,
    pub name: &'static str,
    pub description: &'static str,
    pub argument_hint: Option<&'static str>,
}

impl CommandDef {
    pub const fn new(
        id: CommandId,
        name: &'static str,
        description: &'static str,
        argument_hint: Option<&'static str>,
    ) -> Self {
        Self {
            id,
            name,
            description,
            argument_hint,
        }
    }
}

/// The full registry. Order matches `builtin.json`.
pub const REGISTRY: &[CommandDef] = &[
    CommandDef::new(CommandId::Settings, "settings", "Open settings menu", None),
    CommandDef::new(
        CommandId::Model,
        "model",
        "Select model (opens selector UI)",
        Some("<provider/model>"),
    ),
    CommandDef::new(
        CommandId::Tree,
        "tree",
        "Navigate session tree (switch branches)",
        None,
    ),
    CommandDef::new(
        CommandId::Thinking,
        "thinking",
        "Set thinking level",
        Some("<level>"),
    ),
        CommandDef::new(
        CommandId::ScopedModels,
        "scoped-models",
        "Show models in the current scope",
        None,
    ),
    CommandDef::new(
        CommandId::Status,
        "status",
        "Show current session status",
        None,
    ),
    CommandDef::new(
        CommandId::Help,
        "help",
        "Show help for all commands",
        None,
    ),
    CommandDef::new(
        CommandId::Hotkeys,
        "hotkeys",
        "Show keyboard shortcuts",
        None,
    ),
    CommandDef::new(
        CommandId::Debug,
        "debug",
        "Toggle debug logging",
        None,
    ),
    CommandDef::new(
        CommandId::Editor,
        "editor",
        "Open external editor on the current input buffer",
        None,
    ),
    CommandDef::new(
        CommandId::Login,
        "login",
        "Authenticate with a provider",
        Some("<provider>"),
    ),
    CommandDef::new(
        CommandId::Logout,
        "logout",
        "Sign out of a provider",
        Some("<provider>"),
    ),
    CommandDef::new(
        CommandId::Share,
        "share",
        "Share the current session (export)",
        None,
    ),
    CommandDef::new(
        CommandId::Changelog,
        "changelog",
        "Show changelog",
        None,
    ),
    CommandDef::new(
        CommandId::Trust,
        "trust",
        "Toggle project trust mode",
        None,
    ),
    CommandDef::new(
        CommandId::New,
        "new",
        "Start a new session",
        None,
    ),
    CommandDef::new(
        CommandId::Resume,
        "resume",
        "Resume a previous session",
        Some("<session-id>"),
    ),
    CommandDef::new(
        CommandId::Export,
        "export",
        "Export current session to HTML",
        Some("[path]"),
    ),
    CommandDef::new(
        CommandId::Import,
        "import",
        "Import a session from JSONL",
        Some("<path>"),
    ),
    CommandDef::new(
        CommandId::Fork,
        "fork",
        "Fork the current session",
        None,
    ),
    CommandDef::new(
        CommandId::Clone,
        "clone",
        "Clone a session into a new branch",
        Some("<session-id>"),
    ),
    CommandDef::new(
        CommandId::Compact,
        "compact",
        "Compact the current session",
        None,
    ),
    CommandDef::new(
        CommandId::Session,
        "session",
        "Show session info",
        None,
    ),
CommandDef::new(
        CommandId::Name,
        "name",
        "Name the current session",
        Some("<name>"),
    ),
    CommandDef::new(
        CommandId::Copy,
        "copy",
        "Copy last assistant message to clipboard",
        None,
    ),
    CommandDef::new(
        CommandId::Reload,
        "reload",
        "Reload extensions + skills + settings",
        None,
    ),
    CommandDef::new(
        CommandId::Prompt,
        "prompt",
        "Run a prompt template",
        Some("<name> [args]"),
    ),
    CommandDef::new(
        CommandId::Quit,
        "quit",
        "Quit (with confirmation)",
        None,
    ),
];

/// Look up a `CommandDef` by its slash-command name (without the leading `/`).
pub fn by_name(name: &str) -> Option<&'static CommandDef> {
    REGISTRY.iter().find(|c| c.name == name)
}

/// Score one command against a `/<query>` prefix. Higher = better.
fn score(def: &CommandDef, q: &str) -> i32 {
    let name = def.name;
    if name == q {
        return 1000;
    }
    if name.starts_with(q) {
        return 500 + (name.len() as i32 - q.len() as i32).abs();
    }
    if name.contains(q) {
        return 100;
    }
    0
}

/// Rank `REGISTRY` by prefix / substring match against `query`.
/// Returns at most `limit` results, highest-score first. For an
/// empty query, returns ALL commands (the popup's scroll_offset
/// clips to the viewport; v0.5 hard-capped at 8 and hid 20+ cmds).
pub fn complete(query: &str, limit: usize) -> Vec<&'static CommandDef> {
    if query.is_empty() || query == "/" {
        // Empty query: return ALL commands, not just the first `limit`.
        // The popup's scroll_offset will clip to the viewport, so users
        // can scroll past the first 8 with arrow keys. v0.5 hard-capped
        // at 8 and hid 20+ commands; v0.6 makes everything reachable.
        return REGISTRY.iter().collect();
    }
    let q = query.trim_start_matches('/').to_lowercase();
    let mut scored: Vec<(i32, &'static CommandDef)> = REGISTRY
        .iter()
        .filter_map(|c| {
            let lower = c.name.to_lowercase();
            if lower.starts_with(&q) {
                Some((1000 - c.name.len() as i32, c))
            } else if lower.contains(&q) {
                Some((100 - c.name.len() as i32, c))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().take(limit).map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_28_commands() {
        assert_eq!(
            REGISTRY.len(),
            28,
            "expected 28 commands (23 Pi builtin + 1 nini /prompt + 4 v0.6 fixes)"
        );
    }

    #[test]
    fn by_name_finds_known_commands() {
        for name in &["settings", "model", "quit", "help"] {
            assert!(by_name(name).is_some(), "missing command /{name}");
        }
    }

    #[test]
    fn by_name_returns_none_for_unknown() {
        assert!(by_name("not_a_real_command").is_none());
    }

    #[test]
    fn complete_prefix_match_wins() {
        let r = complete("mo", 8);
        assert!(r.iter().any(|c| c.name == "model"));
        assert!(r[0].name == "model", "model should be first for prefix 'mo'");
    }

    #[test]
    fn complete_exact_match_is_highest() {
        let r = complete("quit", 8);
        assert_eq!(r[0].name, "quit");
    }

    #[test]
    fn complete_empty_returns_all_regardless_of_limit() {
        // v0.6 behavior: empty query returns ALL commands; the popup's
        // scroll_offset handles viewport clipping.
        let r = complete("", 5);
        assert_eq!(r.len(), 28, "empty query should return all 28 commands");
    }

    #[test]
    fn complete_prefix_respects_limit() {
        // With a prefix, complete() returns at most `limit` results.
        let r = complete("set", 5);
        assert!(r.len() <= 5, "limit not respected: got {} items", r.len());
        assert!(!r.is_empty(), "expected some matches for prefix 'set'");
    }

    #[test]
    fn complete_strips_leading_slash() {
        let r = complete("/mo", 8);
        assert!(r.iter().any(|c| c.name == "model"));
    }

    #[test]
    fn all_required_command_ids_present() {
        for name in [
            "settings", "model", "tree", "thinking", "export", "import", "session", "hotkeys",
            "fork", "clone", "trust", "new", "compact", "resume", "reload", "prompt", "quit",
        ] {
            assert!(by_name(name).is_some(), "missing command /{name}");
        }
    }
}