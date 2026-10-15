//! Oracle: a project marked untrusted must actually stop the tools.
//!
//! # What was true before
//!
//! `/trust` writes a decision, `ProjectTrustStore` persists it atomically at
//! 0600, and `check()` turns it into allow/deny. `grep -rn '\.check('` over
//! the workspace returned the definition and its own unit tests — **no
//! production caller**. The `BeforeExecute` hook the gate was meant to ride
//! on had no production implementation either, though the agent loop does
//! consult it (`agent.rs:2019`, phase 2 of `execute_one`).
//!
//! The result: marking a directory untrusted changed the transcript and
//! nothing else. Every `bash`, `write` and `edit` still ran. A security
//! control that displays but does not enforce is worse than an absent one,
//! because it converts "I marked this directory unsafe" into a belief.
//!
//! # What these tests drive
//!
//! The real gate, the real wrapped tool, the real registry — and then the
//! agent loop's own before-execute phase, reached the way it is reached in
//! production (`tool.before()` → run → deny).

use nini_core::project_trust::{ProjectTrustStore, TrustDecision};
use nini_core::tool::{ToolContext, ToolOutput, ToolRegistry};
use std::sync::Arc;

/// Build a registry with one harmless tool, gated for `cwd` under a store
/// whose decision for that cwd is `decision`.
fn gated_registry(decision: TrustDecision, cwd: &str) -> ToolRegistry {
    let mut store = ProjectTrustStore::default();
    store.set(cwd, decision);
    let gate: Arc<dyn nini_core::tool::BeforeExecute> =
        Arc::new(nini_core::trust_gate::ProjectTrustGate::new(
            Arc::new(std::sync::Mutex::new(store)),
            std::path::Path::new(cwd),
            "write",
        ));
    let mut reg = ToolRegistry::new();
    reg.register_mut(Arc::new(nini_core::tool::WrappedTool::new(
        Arc::new(RecordingTool),
        Some(gate),
        None,
    )));
    reg
}

struct RecordingTool;

#[async_trait::async_trait]
impl nini_core::tool::Tool for RecordingTool {
    fn name(&self) -> &'static str {
        "write"
    }
    fn spec(&self) -> nini_core::tool::ToolSpec {
        nini_core::tool::ToolSpec {
            name: "write".into(),
            description: "test".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, nini_core::tool::ToolError> {
        Ok(ToolOutput {
            content: "WROTE".into(),
            is_error: false,
            details: None,
            duration_ms: 0,
            terminate: false,
        })
    }
}

/// Run a tool the way `agent.rs:execute_one` does: before-hook first, then
/// the body, and a denial reported as an error output rather than a panic.
async fn run_gated(reg: &ToolRegistry, args: serde_json::Value, cwd: &str) -> ToolOutput {
    let tool = reg.get("write").expect("tool registered");
    let ctx = ToolContext {
        cwd: std::path::PathBuf::from(cwd),
        ..ToolContext::default()
    };

    let mut denied: Option<String> = None;
    let mut call_args = args.clone();
    if let Some(b) = tool.before() {
        use futures_util::FutureExt;
        match std::panic::AssertUnwindSafe(b.run(args, &ctx))
            .catch_unwind()
            .await
        {
            Ok(Ok(Some(replaced))) => call_args = replaced,
            Ok(Ok(None)) => {}
            Ok(Err(e)) => denied = Some(e.to_string()),
            Err(_) => denied = Some("hook panicked".into()),
        }
    }
    if let Some(msg) = denied {
        return ToolOutput::err(format!("[before-hook denied] {msg}"));
    }
    match tool.execute(call_args, ctx).await {
        Ok(o) => o,
        Err(e) => ToolOutput::err(e.to_string()),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_distrusted_project_cannot_run_tools() {
    let cwd = "/tmp/project-marked-untrusted";
    let reg = gated_registry(TrustDecision::Distrusted, cwd);
    let out = run_gated(&reg, serde_json::json!({"path": "a.rs"}), cwd).await;

    assert!(
        out.is_error,
        "a distrusted project ran the tool anyway: {out:?}"
    );
    assert!(
        out.content.contains("/trust"),
        "the denial must tell the user how to change it; got: {}",
        out.content
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_trusted_project_runs_normally() {
    let cwd = "/tmp/project-marked-trusted";
    let reg = gated_registry(TrustDecision::Trusted, cwd);
    let out = run_gated(&reg, serde_json::json!({"path": "a.rs"}), cwd).await;

    assert!(!out.is_error, "a trusted project was blocked: {out:?}");
    assert_eq!(out.content, "WROTE");
}

/// `Never` is the strongest denial and must not be reachable by the default
/// level of an unrelated directory.
#[tokio::test(flavor = "current_thread")]
async fn never_is_denied() {
    let cwd = "/tmp/project-never";
    let reg = gated_registry(TrustDecision::Never, cwd);
    let out = run_gated(&reg, serde_json::json!({}), cwd).await;
    assert!(out.is_error, "a `never` project ran a tool: {out:?}");
}

/// The gate must not fire for a decision the user never made: the default
/// is `Ask`, and `Ask` allows (it has no place to put a prompt).
#[tokio::test(flavor = "current_thread")]
async fn an_unset_project_is_not_blocked() {
    let cwd = "/tmp/project-never-heard-of-it";
    let reg = gated_registry(TrustDecision::Ask, cwd);
    let out = run_gated(&reg, serde_json::json!({}), cwd).await;
    assert!(
        !out.is_error,
        "the default trust level blocked a tool: {out:?}"
    );
}

/// Trust is keyed on the **session** directory, not on the path an argument
/// happens to name — otherwise "untrust this repo" evaporates the moment a
/// call reads one file from somewhere else.
#[tokio::test(flavor = "current_thread")]
async fn trust_follows_the_session_directory_not_the_argument() {
    let session = "/tmp/project-marked-untrusted";
    let reg = gated_registry(TrustDecision::Distrusted, session);
    // The tool context points somewhere else entirely.
    let out = run_gated(&reg, serde_json::json!({"path": "/etc/passwd"}), "/etc").await;
    assert!(
        out.is_error,
        "the gate keyed on the argument path and let a distrusted project's \
         call through: {out:?}"
    );
}
