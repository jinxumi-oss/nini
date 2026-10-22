//! Project trust store + per-tool-call decision interface.
//!
//! v0.5 introduced `ProjectTrustStore` as a placeholder keyed by
//! cwd; v0.7 (M2) extends it with `check()` that combines the
//! per-cwd decision with per-tool-call policy to produce a binary
//! `TrustDecision::{Allow, Deny}`. The `Ask` half is deferred to
//! v0.8 because it requires cross-task async user confirmation
//! (see `docs/plans/v0.7-hook-parity.md` §M2).

//! invariant: a0d18027 —— 声明本文件守住了哪几条历史不变量。
//! 该标注让 scripts/invariants.py 认出这些断言；词法重叠看不出它们，
//! 真正的证据是 docs/invariant-verification.md 里的注入-观察记录。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustLevel {
    Ask,
    Trusted,
    Distrusted,
    Never,
}

impl Default for TrustLevel {
    fn default() -> Self {
        Self::Ask
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TrustDecision {
    pub level: TrustLevel,
}

impl TrustDecision {
    // PascalCase constants mirror the `TrustLevel` enum variants 1:1
    // so call sites read like a domain concept (`TrustDecision::Trusted`)
    // rather than a magic bit flag. The intentional PascalCase triggers
    // clippy's `non_upper_case_globals`; silenced locally rather than
    // renamed, because the API is consumed at multiple call sites
    // (see dispatch.rs `complete_trust`).
    #[allow(non_upper_case_globals)]
    pub const Trusted: Self = Self {
        level: TrustLevel::Trusted,
    };
    #[allow(non_upper_case_globals)]
    pub const Distrusted: Self = Self {
        level: TrustLevel::Distrusted,
    };
    #[allow(non_upper_case_globals)]
    pub const Ask: Self = Self {
        level: TrustLevel::Ask,
    };
    #[allow(non_upper_case_globals)]
    pub const Never: Self = Self {
        level: TrustLevel::Never,
    };

    pub fn new(level: TrustLevel) -> Self {
        Self { level }
    }

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

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ProjectTrustStore {
    /// The decision for a cwd with no entry of its own.
    #[serde(default)]
    pub decision: TrustDecision,
    #[serde(default)]
    cwd_map: HashMap<String, TrustDecision>,
}

impl ProjectTrustStore {
    /// Read the store.
    ///
    /// v0.7 through v0.10 had this as `load(_path) { Ok(Self::default()) }` —
    /// it ignored the path and never read the file, so `/trust` showed the
    /// user a per-project selector, reported success, and remembered
    /// nothing. A missing file is a legitimate empty store; a *malformed*
    /// one is an error, because silently treating an unreadable
    /// security decision as "nothing is trusted" is not a default any
    /// more than a zero cost is.
    pub fn load(path: &Path) -> Result<Self, std::io::Error> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{}: {e}", path.display()),
                )
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// The decision in force for `cwd`: its own if it has one, otherwise
    /// the store-wide default.
    pub fn get(&self, key: &str) -> Option<TrustDecision> {
        Some(self.get_for_cwd(key))
    }

    /// `~/.nini/agent/trust.json`, or wherever `NINI_AGENT_DIR` points.
    ///
    /// The previous version returned the *relative* `.nini/agent/trust.json`,
    /// which resolved against the working directory rather than the home
    /// directory — so the same decision landed in a different file
    /// depending on where nini was launched from.
    pub fn default_path() -> Option<std::path::PathBuf> {
        crate::paths::agent_file("trust.json")
    }

    /// Write the store atomically, at 0600 on unix.
    ///
    /// Trust decisions are security state: a partially written file, or one
    /// any local user can rewrite, is a store that cannot be relied on.
    /// Write to a sibling temp file and rename, which is atomic within a
    /// filesystem, then tighten the mode before the rename so the file is
    /// never briefly world-readable.
    pub fn save(&self, path: &Path) -> Result<(), std::io::Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, body.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&tmp, mode)?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Forget one project's decision. Previously a no-op, so there was no
    /// way back to "ask me again" once a directory had been trusted.
    pub fn clear(&mut self, cwd: &str) {
        self.cwd_map.remove(cwd);
    }

    pub fn set(&mut self, cwd: &str, decision: TrustDecision) {
        self.cwd_map.insert(cwd.to_string(), decision);
    }

    /// v0.7 (M2) — look up the per-cwd decision.
    pub fn get_for_cwd(&self, cwd: &str) -> TrustDecision {
        self.cwd_map.get(cwd).copied().unwrap_or(self.decision)
    }

    /// v0.7 (M2) — combined per-cwd + per-tool decision.
    ///
    /// Returns `Allow` if the cwd is trusted, `Deny` if the cwd is
    /// distrusted or "never". Tool-specific policy is layered on top
    /// in v0.8; for now every tool inherits the cwd decision.
    pub fn check(&self, call: &ToolCall) -> BinaryDecision {
        let (binary, warning) = self.check_and_warn(call);
        if let Some(warning) = warning {
            eprintln!("{warning}");
        }
        binary
    }

    /// The decision **and** the warning, without printing anything.
    ///
    /// The caller that owns the store's lock must not print while it
    /// holds it: the TUI's terminal guard makes stderr a pipe, and a
    /// pipe that nobody reads fills up and blocks. A blocked `eprintln!`
    /// inside the lock would stall every other tool call that waits
    /// for the same lock — the agent would look frozen, and only
    /// Ctrl+C would not help because the lock holder is the one stuck.
    ///
    /// So the warning is *returned*, and the caller prints it after
    /// the lock is gone.
    pub fn check_and_warn(&self, call: &ToolCall) -> (BinaryDecision, Option<String>) {
        let cwd_decision = self.get_for_cwd(&call.cwd);
        // For v0.7 we don't differentiate per-tool. The Ask level
        // collapses to Allow (with a log warn emitted by the caller).
        let binary = cwd_decision.to_binary();
        (binary, self.ask_level_warning_for(call))
    }

    /// The warning this call produces — if any.
    ///
    /// `None` for a project that is trusted or distrusted: there is
    /// nothing to say about those, and saying something would be a
    /// lie. `None` for a cwd whose warning was already stated (see
    /// [`ask_level_warning`]).
    ///
    /// It is a separate function so that "stays quiet" is a thing a
    /// test can assert, instead of a thing the reader has to believe.
    fn ask_level_warning_for(&self, call: &ToolCall) -> Option<String> {
        if !matches!(self.get_for_cwd(&call.cwd).level, TrustLevel::Ask) {
            return None;
        }
        ask_level_warning(&call.cwd)
    }
}

/// The warning the `Ask` level produces — **once per cwd**.
///
/// Why once: `check()` runs before *every* tool call, and the `Ask`
/// level is the default when there is no `trust.json`. So the warning
/// was a fact about the *tool call* and repeated as often as the agent
/// ran tools — twenty tool calls, twenty identical lines.
///
/// Since v0.10.3 those lines are not scribbled on the screen any more
/// (the TUI turns stderr into transcript notices), so the repetition
/// became visible as twenty identical rows in the transcript. The
/// warning is a fact about the **project's** trust posture, not about
/// each call, so it is stated once per cwd.
fn ask_level_warning(cwd: &str) -> Option<String> {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};

    static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let warned = WARNED.get_or_init(|| Mutex::new(HashSet::new()));
    {
        let mut set = warned
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !set.insert(cwd.to_string()) {
            return None;
        }
    }
    Some(format!(
        "[nini] trust: cwd '{cwd}' is at Ask level; defaulting to Allow (v0.8 will add interactive prompt)"
    ))
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
    fn the_ask_warning_is_stated_once_per_cwd() {
        // The warning is a fact about the project's trust posture, not
        // about each tool call. `check()` runs before every call, and
        // `Ask` is the default when there is no trust.json — so a
        // twenty-tool-call task produced twenty identical lines (and,
        // since v0.10.3, twenty identical rows in the transcript).
        let cwd = "/tmp/nini-trust-warn-once-probe";
        let first = ask_level_warning(cwd);
        assert!(
            first.is_some(),
            "the first call for a cwd must state the warning"
        );
        let warning = first.expect("checked above");
        assert!(
            warning.contains(cwd),
            "the warning must name the cwd it is about: {warning}"
        );
        assert!(
            ask_level_warning(cwd).is_none(),
            "the second call for the same cwd must not repeat the warning"
        );
        // A different cwd is a different project: its own warning.
        let other = "/tmp/nini-trust-warn-once-probe-2";
        assert!(
            ask_level_warning(other).is_some(),
            "a different cwd must get its own warning"
        );
    }

    #[test]
    fn a_trusted_or_distrusted_cwd_never_asks() {
        // The warning is only about the Ask level. A trusted project
        // must stay silent, or the transcript fills with a warning that
        // says nothing true — and a distrusted one must stay silent
        // too: it is not being asked anything, it is being refused.
        let trusted = trusted_store();
        assert!(
            trusted
                .ask_level_warning_for(&ToolCall {
                    tool: "bash",
                    args: Value::Null,
                    cwd: "/home/me/proj".to_string(),
                })
                .is_none(),
            "a trusted project has nothing to warn about"
        );
        let distrusted = distrusted_store();
        assert!(
            distrusted
                .ask_level_warning_for(&ToolCall {
                    tool: "bash",
                    args: Value::Null,
                    cwd: "/tmp/danger".to_string(),
                })
                .is_none(),
            "a distrusted project is refused, not warned about"
        );
        // And the Ask level does warn — otherwise "stays quiet" would
        // be true for the wrong reason.
        //
        // A cwd of its own: the warning is stated once per cwd for the
        // whole process, and the other tests in this binary have
        // already spent the one for "/home/me/ask".
        let asked_cwd = "/tmp/nini-trust-asks-warning-probe";
        let mut asked = ProjectTrustStore::default();
        asked.set(asked_cwd, TrustDecision::Ask);
        assert!(
            asked
                .ask_level_warning_for(&ToolCall {
                    tool: "bash",
                    args: Value::Null,
                    cwd: asked_cwd.to_string(),
                })
                .is_some(),
            "the Ask level must state the warning"
        );
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
        assert_eq!(TrustDecision::Distrusted.to_binary(), BinaryDecision::Deny);
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
