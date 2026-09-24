//! Project trust store + per-tool-call decision interface.
//!
//! v0.5 introduced `ProjectTrustStore` as a placeholder keyed by
//! cwd; v0.7 (M2) extends it with `check()` that combines the
//! per-cwd decision with per-tool-call policy to produce a binary
//! `TrustDecision::{Allow, Deny}`. The `Ask` half is deferred to
//! v0.8 because it requires cross-task async user confirmation
//! (see `docs/plans/v0.7-hook-parity.md` §M2).

use std::collections::HashMap;
use std::path::Path;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustLevel {
    Ask,
    Trusted,
    Distrusted,
    Never,
}

impl Default for TrustLevel {
    fn default() -> Self { Self::Ask }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrustDecision {
    pub level: TrustLevel,
}

impl TrustDecision {
    pub const Trusted: Self = Self { level: TrustLevel::Trusted };
    pub const Distrusted: Self = Self { level: TrustLevel::Distrusted };
    pub const Ask: Self = Self { level: TrustLevel::Ask };
    pub const Never: Self = Self { level: TrustLevel::Never };

    pub fn new(level: TrustLevel) -> Self { Self { level } }

    /// v0.7 (M2) — collapse the 4-state trust model to a 2-state
    /// decision suitable for the before-execute hook. The rule:
    ///   * `Trusted`           → Allow
    ///   * `Distrusted`        → Deny
    ///   * `Never`             → Deny
    ///   * `Ask`               → Allow (with a warn; v0.8 will replace
    ///                            this with an interactive prompt)
    pub fn to_binary(&self) -> BinaryDecision {
        match self.level {
            TrustLevel::Trusted => BinaryDecision::Allow,
            TrustLevel::Ask => BinaryDecision::Allow,
            TrustLevel::Distrusted => BinaryDecision::Deny,
            TrustLevel::Never => BinaryDecision::Deny,
        }
    }
}

/// v0.7 (M2) — 2-state decision returned by `ProjectTrustStore::check`.
/// `Ask` is deliberately absent: it requires async user input that the
/// hook pipeline can't deliver (yet). See plan §M2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryDecision {
    Allow,
    Deny,
}

/// v0.7 (M2) — input to `check()`. Captures the per-call context
/// (tool name + JSON args) so the trust decision can be
/// arg-sensitive (e.g. "allow read inside project, deny read of
/// /etc/passwd").
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub tool: &'static str,
    pub args: Value,
    pub cwd: String,
}

#[derive(Default)]
pub struct ProjectTrustStore {
    pub decision: TrustDecision,
    cwd_map: HashMap<String, TrustDecision>,
}

impl ProjectTrustStore {
    pub fn load(_path: &Path) -> Result<Self, std::io::Error> {
        Ok(Self::default())
    }
    pub fn get(&self, _key: &str) -> Option<TrustDecision> {
        Some(self.decision.clone())
    }
    pub fn default_path() -> Option<std::path::PathBuf> {
        Some(std::path::PathBuf::from(".pi/agent/trust.json"))
    }

    pub fn save(&self, _path: &Path) -> Result<(), std::io::Error> { Ok(()) }
    pub fn clear(&mut self, _cwd: &str) {}

    pub fn set(&mut self, cwd: &str, decision: TrustDecision) {
        self.cwd_map.insert(cwd.to_string(), decision);
    }

    /// v0.7 (M2) — look up the per-cwd decision.
    pub fn get_for_cwd(&self, cwd: &str) -> TrustDecision {
        self.cwd_map
            .get(cwd)
            .copied()
            .unwrap_or(self.decision)
    }

    /// v0.7 (M2) — combined per-cwd + per-tool decision.
    ///
    /// Returns `Allow` if the cwd is trusted, `Deny` if the cwd is
    /// distrusted or "never". Tool-specific policy is layered on top
    /// in v0.8; for now every tool inherits the cwd decision.
    pub fn check(&self, call: &ToolCall) -> BinaryDecision {
        let cwd_decision = self.get_for_cwd(&call.cwd);
        // For v0.7 we don't differentiate per-tool. The Ask level
        // collapses to Allow (with a log warn emitted by the caller).
        let binary = cwd_decision.to_binary();
        if matches!(cwd_decision.level, TrustLevel::Ask) {
            eprintln!(
                "[nini] trust: cwd '{}' is at Ask level; defaulting to Allow (v0.8 will add interactive prompt)",
                call.cwd
            );
        }
        binary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trusted_store() -> ProjectTrustStore {
        let mut s = ProjectTrustStore::default();
        s.set("/home/me/proj", TrustDecision::Trusted);
        s
    }

    fn distrusted_store() -> ProjectTrustStore {
        let mut s = ProjectTrustStore::default();
        s.set("/tmp/danger", TrustDecision::Distrusted);
        s
    }

    fn ask_store() -> ProjectTrustStore {
        let mut s = ProjectTrustStore::default();
        s.set("/home/me/ask", TrustDecision::Ask);
        s
    }

    #[test]
    fn trust_level_default_is_ask() {
        assert_eq!(TrustLevel::default(), TrustLevel::Ask);
    }

    #[test]
    fn trust_decision_constants() {
        assert_eq!(TrustDecision::Trusted.level, TrustLevel::Trusted);
        assert_eq!(TrustDecision::Distrusted.level, TrustLevel::Distrusted);
        assert_eq!(TrustDecision::Ask.level, TrustLevel::Ask);
        assert_eq!(TrustDecision::Never.level, TrustLevel::Never);
    }

    #[test]
    fn to_binary_collapses_four_to_two() {
        assert_eq!(TrustDecision::Trusted.to_binary(), BinaryDecision::Allow);
        assert_eq!(TrustDecision::Ask.to_binary(), BinaryDecision::Allow);
        assert_eq!(
            TrustDecision::Distrusted.to_binary(),
            BinaryDecision::Deny
        );
        assert_eq!(TrustDecision::Never.to_binary(), BinaryDecision::Deny);
    }

    #[test]
    fn check_returns_allow_for_trusted_cwd() {
        let store = trusted_store();
        let call = ToolCall {
            tool: "bash",
            args: serde_json::json!({"command": "ls"}),
            cwd: "/home/me/proj".into(),
        };
        assert_eq!(store.check(&call), BinaryDecision::Allow);
    }

    #[test]
    fn check_returns_deny_for_distrusted_cwd() {
        let store = distrusted_store();
        let call = ToolCall {
            tool: "bash",
            args: serde_json::json!({"command": "rm -rf /"}),
            cwd: "/tmp/danger".into(),
        };
        assert_eq!(store.check(&call), BinaryDecision::Deny);
    }

    #[test]
    fn check_defaults_to_default_decision_when_cwd_unknown() {
        let store = ProjectTrustStore::default(); // default = Ask
        let call = ToolCall {
            tool: "read",
            args: serde_json::json!({"path": "/etc/hosts"}),
            cwd: "/some/unknown/cwd".into(),
        };
        // Default collapses to Allow + warn log.
        assert_eq!(store.check(&call), BinaryDecision::Allow);
    }

    #[test]
    fn check_returns_allow_for_ask_cwd_with_warn() {
        let store = ask_store();
        let call = ToolCall {
            tool: "bash",
            args: serde_json::json!({"command": "ls"}),
            cwd: "/home/me/ask".into(),
        };
        // The warn line goes to stderr; the decision is Allow for v0.7.
        assert_eq!(store.check(&call), BinaryDecision::Allow);
    }

    #[test]
    fn check_unregistered_cwd_uses_default_decision() {
        let mut store = ProjectTrustStore::default();
        // Override the global default to Distrusted.
        store.decision = TrustDecision::Distrusted;
        let call = ToolCall {
            tool: "bash",
            args: serde_json::json!({}),
            cwd: "/never/seen/before".into(),
        };
        // Unregistered cwd should pick up the global default
        // (Distrusted → Deny).
        assert_eq!(store.check(&call), BinaryDecision::Deny);
    }

    #[test]
    fn tool_call_carries_tool_name_args_and_cwd() {
        let call = ToolCall {
            tool: "read",
            args: serde_json::json!({"path": "Cargo.toml"}),
            cwd: "/home/me/proj".into(),
        };
        assert_eq!(call.tool, "read");
        assert_eq!(call.cwd, "/home/me/proj");
        assert_eq!(call.args["path"], "Cargo.toml");
    }
}