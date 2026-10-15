//! v0.10 (fix): enforce the project trust decision the user already set.
//!
//! `ProjectTrustStore` has been fully implemented since v0.9.2 — atomic
//! 0600 writes, a per-cwd map, a default level, a `check()` that collapses
//! four trust levels to allow/deny — and `/trust` has been wired to it in
//! both directions (select a level, persist it, read it back).
//!
//! What never happened: **anything asked the store during a run.**
//! `grep -rn '\.check(' crates/` returned the definition and its own unit
//! tests, and nothing else. The `BeforeExecute` surface that exists
//! precisely to host such a gate (`tool.rs:285`) had no production
//! implementation either, even though the agent loop does consult it
//! (`agent.rs:2019`, phase 2 of `execute_one`).
//!
//! So the user could mark a directory untrusted, watch `/trust` confirm it,
//! and then watch the agent read, write and `bash` in that directory anyway.
//! A security control that displays but does not enforce is worse than one
//! that is absent: it turns "I marked this directory unsafe" into a belief.
//!
//! # What this gate is, and what it is not
//!
//! - **It is** the per-directory allow/deny the user configured, applied to
//!   every built-in tool call.
//! - **It is not** a path sandbox. `Ask` (the default) still allows, because
//!   an interactive prompt cannot be delivered from inside a synchronous hook
//!   — the same reason `TrustDecision::to_binary` collapses it, unchanged.
//! - Trust is keyed on the **session directory**, not on whatever path a
//!   call happens to name. `/trust` is a per-project decision; honouring it
//!   per-argument would make "untrust this repo" meaningless the moment a
//!   tool read one file from elsewhere.
//!
//! # Wiring, in one place
//!
//! `apply_project_trust` wraps each tool in a `WrappedTool` carrying the
//! gate. Called from `app.rs` right after the CLI tool filters, so it sees
//! exactly the tool set the run will use.

use crate::project_trust::{BinaryDecision, ProjectTrustStore, ToolCall};
use crate::tool::{
    AfterExecute, BeforeExecute, ToolContext, ToolError, ToolOutput, ToolRegistry, WrappedTool,
};
use std::sync::Arc;

/// v0.10 (fix): the gate itself, bound to one tool's name.
pub struct ProjectTrustGate {
    pub(crate) store: Arc<std::sync::Mutex<ProjectTrustStore>>,
    /// The directory the session was started in.
    pub(crate) session_cwd: String,
    /// The tool this instance guards. `&'static` because
    /// `ToolCall::tool` is `&'static str` and every built-in tool already
    /// satisfies that bound.
    pub(crate) tool: &'static str,
}

impl ProjectTrustGate {
    /// Build a gate for one tool in one session directory.
    pub fn new(
        store: Arc<std::sync::Mutex<ProjectTrustStore>>,
        session_cwd: &std::path::Path,
        tool: &'static str,
    ) -> Self {
        Self {
            store,
            session_cwd: session_cwd.display().to_string(),
            tool,
        }
    }
}

#[async_trait::async_trait]
impl BeforeExecute for ProjectTrustGate {
    async fn run(
        &self,
        args: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<Option<serde_json::Value>, ToolError> {
        let decision = {
            let store = self
                .store
                .lock()
                .map_err(|_| ToolError::InvalidArgs("project trust store is poisoned".into()))?;
            store.check(&ToolCall {
                tool: self.tool,
                args: args.clone(),
                cwd: self.session_cwd.clone(),
            })
        };

        match decision {
            BinaryDecision::Allow => Ok(None),
            BinaryDecision::Deny => Err(ToolError::PermissionDenied(format!(
                "this project is marked untrusted, so `{}` is not allowed to \
                 run here. Use /trust to change the decision.",
                self.tool
            ))),
        }
    }
}

struct PassThrough;

#[async_trait::async_trait]
impl AfterExecute for PassThrough {
    async fn run(&self, output: ToolOutput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(output)
    }
}

/// Wrap every tool in `registry` with the project-trust gate.
///
/// With no trust store on disk the registry comes back unchanged, so a user
/// who has never run `/trust` keeps today's behaviour: `Ask` → allow.
pub fn apply_project_trust(registry: ToolRegistry, session_cwd: &std::path::Path) -> ToolRegistry {
    let Some(store) = load_store() else {
        return registry;
    };

    let mut out = ToolRegistry::new();
    for tool in registry.tools().collect::<Vec<_>>() {
        let gate: Arc<dyn BeforeExecute> = Arc::new(ProjectTrustGate::new(
            store.clone(),
            session_cwd,
            tool.name(),
        ));
        out.register_mut(Arc::new(WrappedTool::new(
            tool,
            Some(gate),
            Some(Arc::new(PassThrough)),
        )));
    }
    out
}

fn load_store() -> Option<Arc<std::sync::Mutex<ProjectTrustStore>>> {
    let path = ProjectTrustStore::default_path()?;
    match ProjectTrustStore::load(&path) {
        Ok(store) => Some(Arc::new(std::sync::Mutex::new(store))),
        Err(e) => {
            // A store that exists but cannot be parsed is security state, not
            // a missing file. Failing open here would mean a corrupt file
            // silently disables the user's decision, so say so loudly and
            // fall back to the default (`Ask` → allow) rather than inventing
            // a denial that the user never asked for.
            eprintln!(
                "[nini] trust store at {} could not be read ({e}); falling \
                 back to the default trust level",
                path.display()
            );
            Some(Arc::new(
                std::sync::Mutex::new(ProjectTrustStore::default()),
            ))
        }
    }
}
