//! Tool trait, spec, output, context, error, and registry.
//!
//! Concrete tool impls (Bash, Read, etc.) live in `nini-tools`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

/// Static specification of a tool (name + description + JSON schema).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// v0.7.1 (Pi hook #9) — a tool's contribution to the system prompt.
///
/// Each tool can provide a short `snippet` (model-facing intro) and a
/// list of `guidelines` (usage rules). The agent loop gathers these
/// from all registered tools at request-build time and appends them
/// to the user's `RunConfig.system` prompt.
///
/// Different from `ToolSpec.description`:
///   * `ToolSpec.description` is the LONG documentation the model
///     sees in the tool schema at every turn.
///   * `ToolSystemPrompt.snippet` is the SHORT blurb that appears in
///     the system prompt ONCE at conversation start.
///   * `ToolSystemPrompt.guidelines` are per-tool usage rules that
///     appear in the system prompt and are NOT repeated in every
///     tool schema call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolSystemPrompt {
    /// Short description (~1-2 sentences) for the system prompt.
    /// Example: "Execute shell commands via $SHELL -c, return
    /// captured stdout+stderr, support timeout."
    #[serde(default)]
    pub snippet: String,
    /// Usage guidelines ("Be careful with destructive ops", "Use
    /// absolute paths"). Empty Vec means no guidelines.
    #[serde(default)]
    pub guidelines: Vec<String>,
}

impl ToolSystemPrompt {
    /// Convenience: empty (default) contribution. Tools that opt
    /// out of hook #9 can return `Some(Self::empty())` to force
    /// "no contribution" without returning `None`.
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Result of a tool invocation, returned to the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Text content (already-truncated if necessary).
    pub content: String,
    /// True if the tool errored (network, exit non-zero, etc.).
    pub is_error: bool,
    /// Optional structured details (exit code, duration, etc.).
    pub details: Option<Value>,
    /// Wall-clock duration of the tool execution in milliseconds.
    /// Surfaced in the TUI transcript as `Took N.Ns` (Pi-style)
    /// so the user can see how long each tool took. `0` when the
    /// tool does not measure its own duration (e.g. read of a
    /// cached file).
    pub duration_ms: u64,
    /// v0.10 (R1c): this tool has the final answer — end the turn
    /// instead of asking the model again.
    ///
    /// Pi: `AgentToolResult.terminate`, consumed by
    /// `shouldTerminateToolBatch` (`agent-loop.ts:589-584`). A tool that
    /// collects a complete answer sets this so the loop stops rather than
    /// spending another model call to be told what the tool already knows.
    ///
    /// Defaults to false. The batch-level verdict is the **logical and**
    /// over every result — see [`should_terminate_tool_batch`].
    #[serde(default)]
    pub terminate: bool,
}

/// Per-tool execution-mode override. Mirrors Pi's `ToolExecutionMode`
/// (`pi-agent-core/src/types.ts:402-408`).
///
/// v0.15: derives `Serialize`/`Deserialize` so the mode can be read out
/// of a config file. The spelling is `lowercase` — `"parallel"` /
/// `"sequential"` — the same two values Pi's type has
/// (`references/pi-0.85.1/CORE/types.d.ts:22`). The parse is
/// case-sensitive on purpose: Pi accepts no other spelling, and nini
/// growing a third dialect would be drift nobody asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolExecutionMode {
    /// This tool may run concurrently with the rest of the batch.
    #[default]
    Parallel,
    /// This tool must run one at a time, with the other tool calls too.
    Sequential,
}

/// Decide whether a finished tool batch ends the turn.
///
/// Mirrors Pi's `shouldTerminateToolBatch` (`agent-loop.ts:589-584`):
///
/// ```ts
/// return finalizedCalls.length > 0 && finalizedCalls.every((f) => f.result.terminate === true);
/// ```
///
/// The `every` is the load-bearing detail. "any result terminated" would
/// let one finished tool cut off a sibling that still has something to
/// report, and the empty-batch guard stops a batch of zero from ending the
/// turn.
pub fn should_terminate_tool_batch(results: &[bool]) -> bool {
    !results.is_empty() && results.iter().all(|t| *t)
}

impl ToolOutput {
    /// Convenience: a successful text output.
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            details: None,
            duration_ms: 0,
            terminate: false,
        }
    }

    /// Convenience: an error output.
    pub fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            details: None,
            duration_ms: 0,
            // An errored tool never holds a final answer.
            terminate: false,
        }
    }
}

/// Context passed to a tool on each invocation.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Working directory for the tool (typically the session's cwd).
    pub cwd: std::path::PathBuf,
    /// 字节预算。
    ///
    /// v0.10.8 (T4): 之前是 1 MiB —— 比 Pi 大二十倍，于是同一条命令在两侧
    /// 占用的上下文完全不同。Pi 的上限是 50 KiB
    /// （`references/pi-0.85.1/dist/api/...` → tools/truncate.js 的
    /// `DEFAULT_MAX_BYTES = 50 * 1024`）。
    pub max_output_bytes: usize,
    /// 行数预算（2000 行，与 Pi 的 `DEFAULT_MAX_LINES` 一致）。
    pub max_output_lines: usize,
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            max_output_bytes: 50 * 1024, // Pi 0.85.1 的 DEFAULT_MAX_BYTES
            max_output_lines: 2000,      // Pi 0.85.1 的 DEFAULT_MAX_LINES
        }
    }
}

/// Tool error type.
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("execution timed out after {0} ms")]
    Timeout(u64),
    #[error("tool not found: {0}")]
    NotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("internal: {0}")]
    Internal(String),
}

impl From<std::io::Error> for ToolError {
    fn from(e: std::io::Error) -> Self {
        ToolError::Io(e.to_string())
    }
}

/// The `Tool` trait: implementors provide a name, spec, and async execute.
///
/// v0.7 (M2) — extends the trait with optional `before()` / `after()`
/// hook slots so extensions can intercept tool execution without
/// rewriting the tool itself. Both default to `None`, which means
/// "no hook installed" = v0.6.x behavior. Existing tool impls are
/// unaffected.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name as it appears in the model's tool schema.
    fn name(&self) -> &'static str;
    /// Static spec (description + JSON schema).
    fn spec(&self) -> ToolSpec;
    /// Execute the tool with the given (already-validated) arguments.
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError>;

    /// v0.7 (M2) — return a `BeforeExecute` hook to intercept the
    /// tool call BEFORE `execute()` runs. Default: no hook.
    ///
    /// The hook receives the parsed `Value` and the `ToolContext`
    /// and may:
    ///   * Return `Ok(Some(modified_args))` to substitute new args
    ///   * Return `Ok(None)` to pass through unchanged
    ///   * Return `Err(...)` to deny execution with that error
    ///     message (the agent surfaces it as a `ToolResult` with
    ///     `is_error: true`).
    ///
    /// Common use cases: permission checks (TrustStore), path
    /// sanitization, arg rewriting, audit logging.
    /// v0.10 (R1c): per-tool execution-mode override, mirroring Pi's
    /// `AgentTool.executionMode` (`types.ts:402-408`).
    ///
    /// Returning [`ToolExecutionMode::Sequential`] downgrades the **entire
    /// batch** to serial execution, not just this tool — Pi's behaviour at
    /// `agent-loop.ts:416-426`:
    ///
    /// ```ts
    /// const hasSequentialToolCall = toolCalls.some(
    ///     (tc) => tools?.find((t) => t.name === tc.name)?.executionMode === "sequential");
    /// if (config.toolExecution === "sequential" || hasSequentialToolCall) { … }
    /// ```
    ///
    /// The downgrade is all-or-nothing because a sequential tool needs
    /// exclusive use of whatever shared state it touches; interleaving
    /// siblings around it breaks that guarantee just as surely as running
    /// it alongside them.
    ///
    /// Default: [`ToolExecutionMode::Parallel`], which preserves the worker
    /// pool added in v0.8.5.
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::default()
    }

    fn before(&self) -> Option<Box<dyn BeforeExecute>> {
        None
    }

    /// v0.7 (M2) — return an `AfterExecute` hook to intercept the
    /// tool output AFTER `execute()` succeeds. Default: no hook.
    ///
    /// The hook receives the raw `ToolOutput` and may rewrite
    /// `content` (e.g. for redaction), flip `is_error`, or attach
    /// extra `details`.
    ///
    /// NOTE: hooks only fire on the success path. Coll a tool that
    /// returns `Err` from `execute()` is surfaced as-is.
    fn after(&self) -> Option<Box<dyn AfterExecute>> {
        None
    }

    /// v0.7.1 (Pi hook #9) — return this tool's contribution to
    /// the system prompt: a short `snippet` + a list of usage
    /// `guidelines`. The agent loop gathers contributions from
    /// all registered tools and appends them to
    /// `RunConfig.system` at request-build time.
    ///
    /// Default: no contribution (Pi parity behavior for tools that
    /// don't opt in).
    ///
    /// Pi contract: each tool can have its own self-description.
    /// The full system prompt then reads like:
    ///
    /// ```text
    /// <user's system prompt>
    ///
    /// ## Tool self-descriptions
    /// - bash: Execute shell commands...
    /// - read: Read a file's contents...
    ///
    /// ## Tool usage guidelines
    /// - bash: Use absolute paths
    /// - edit: Always provide unique oldText
    /// ```
    fn system_prompt_contribution(&self) -> Option<ToolSystemPrompt> {
        None
    }
}

/// v0.7 (M2) hook trait — runs BEFORE `Tool::execute()`.
///
/// Implementors may return modified args, deny the call, or pass
/// through unchanged. See `Tool::before()` for the contract.
#[async_trait]
pub trait BeforeExecute: Send + Sync {
    async fn run(&self, args: Value, ctx: &ToolContext) -> Result<Option<Value>, ToolError>;
}

/// v0.7 (M2) hook trait — runs AFTER `Tool::execute()` succeeds.
///
/// Implementors may rewrite the output. See `Tool::after()` for the
/// contract.
#[async_trait]
pub trait AfterExecute: Send + Sync {
    async fn run(&self, output: ToolOutput, ctx: &ToolContext) -> Result<ToolOutput, ToolError>;
}

/// Registry of tools, indexed by name.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool. Replaces any existing tool with the same name.
    pub fn register(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.insert(tool.name().to_string(), tool);
        self
    }

    /// In-place registration. Useful when the registry is mutated inside a
    /// loop where chaining would consume the receiver.
    pub fn register_mut(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Iterate over all registered tools (as `Arc<dyn Tool>`).
    pub fn tools(&self) -> impl Iterator<Item = Arc<dyn Tool>> + '_ {
        self.tools.values().cloned()
    }

    /// List all registered tool specs (for the model).
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|t| t.spec()).collect()
    }

    /// List all registered tool names.
    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }
}

/// v0.7 (M2) — `WrappedTool` lets extensions inject before/after
/// hooks around an existing tool without rewriting the tool.
///
/// The wrapper forwards `name()` and `spec()` to the inner tool and
/// only adds the hook slots. The agent loop still drives
/// `execute()` directly on the inner tool, but the wrapper exposes
/// `before()` / `after()` so the run loop can call the hooks around
/// the execute.
///
/// Common pattern: wrap an existing Read tool with a path whitelist
/// `BeforeExecute` to block reads outside the project root.
///
/// Hooks are stored as `Arc<dyn ...>` so the `before()` /
/// `after()` methods can return a fresh `Box<dyn ...>` clone on
/// every call (the trait return type owns the hook).
pub struct WrappedTool {
    inner: Arc<dyn Tool>,
    before_hook: Option<Arc<dyn BeforeExecute>>,
    after_hook: Option<Arc<dyn AfterExecute>>,
}

impl WrappedTool {
    /// Wrap `inner` with optional hooks. Pass `None` for slots you
    /// don't want to override.
    pub fn new(
        inner: Arc<dyn Tool>,
        before_hook: Option<Arc<dyn BeforeExecute>>,
        after_hook: Option<Arc<dyn AfterExecute>>,
    ) -> Self {
        Self {
            inner,
            before_hook,
            after_hook,
        }
    }

    /// Borrow the inner tool (e.g. to call `execute()` directly).
    pub fn inner(&self) -> &Arc<dyn Tool> {
        &self.inner
    }
}

#[async_trait]
impl Tool for WrappedTool {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn spec(&self) -> ToolSpec {
        self.inner.spec()
    }
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        self.inner.execute(args, ctx).await
    }
    fn before(&self) -> Option<Box<dyn BeforeExecute>> {
        // Return a thin adapter that clones the Arc so the call
        // site gets an owned Box<dyn BeforeExecute + 'static>.
        let hook = self.before_hook.as_ref()?.clone();
        let boxed: Box<dyn BeforeExecute> = Box::new(ArcBoxAdapter(hook));
        Some(boxed)
    }
    fn after(&self) -> Option<Box<dyn AfterExecute>> {
        let hook = self.after_hook.as_ref()?.clone();
        let boxed: Box<dyn AfterExecute> = Box::new(ArcBoxAdapterAfter(hook));
        Some(boxed)
    }
}

/// Adapter from `Arc<dyn BeforeExecute>` to `Box<dyn BeforeExecute>`.
/// Wrapping the Arc in a newtype lets us box it on each call without
/// the lifetime problem that a raw `&dyn` adapter would have.
struct ArcBoxAdapter(Arc<dyn BeforeExecute>);
#[async_trait]
impl BeforeExecute for ArcBoxAdapter {
    async fn run(&self, args: Value, ctx: &ToolContext) -> Result<Option<Value>, ToolError> {
        self.0.run(args, ctx).await
    }
}

/// Public re-export of the Arc adapter so test code (and any
/// downstream extension) can construct `Box<dyn BeforeExecute>` from
/// an `Arc<dyn BeforeExecute>` without rolling their own adapter.
pub struct ArcBoxAdapterBefore(pub Arc<dyn BeforeExecute>);
#[async_trait]
impl BeforeExecute for ArcBoxAdapterBefore {
    async fn run(&self, args: Value, ctx: &ToolContext) -> Result<Option<Value>, ToolError> {
        self.0.run(args, ctx).await
    }
}

struct ArcBoxAdapterAfter(Arc<dyn AfterExecute>);
#[async_trait]
impl AfterExecute for ArcBoxAdapterAfter {
    async fn run(&self, output: ToolOutput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        self.0.run(output, ctx).await
    }
}

/// Public re-export of the Arc adapter for `AfterExecute`.
pub struct ArcBoxAdapterAfterTool(pub Arc<dyn AfterExecute>);
#[async_trait]
impl AfterExecute for ArcBoxAdapterAfterTool {
    async fn run(&self, output: ToolOutput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        self.0.run(output, ctx).await
    }
}

/// v0.7.1 — aggregate `system_prompt_contribution()` calls from
/// every tool in `registry` and append them to `base`. Returns
/// `None` if no tool provides a contribution AND `base` is
/// `None`; otherwise returns the assembled string.
///
/// Format (Pi-style):
///
/// ```text
/// <base prompt>
///
/// ## Tool self-descriptions
/// - bash: Execute shell commands...
/// - read: Read a file's contents...
///
/// ## Tool usage guidelines
/// - bash: Use absolute paths
/// - edit: Always provide unique oldText
/// ```
///
/// Snippets are emitted in the order `ToolRegistry::tools()`
/// yields them (currently insertion order — the registry uses a
/// `HashMap` but `register` is monotonic). Empty `snippet`s
/// and empty `guideline`s are skipped.
pub fn build_system_prompt_with_contributions(
    base: Option<&str>,
    registry: &ToolRegistry,
) -> Option<String> {
    // v0.10.8 (T3)：与 Pi 0.85.1 同构（`references/pi-0.85.1/dist/api/system-prompt.js`）。
    //
    // Pi 的三条规则：
    //   1. 「Available tools」段**只列有 snippet 的工具**
    //      （`visibleTools = tools.filter(name => !!toolSnippets?.[name])`），
    //      一个都没有时写 "(none)"；
    //   2. guideline 由**已启用的工具集**决定：
    //      `if ((hasBash || hasPowerShell) && !hasGrep && !hasFind && !hasLs)`
    //      → "Use bash for file operations like ls, rg, find"；
    //   3. 永远有两条："Be concise in your responses" /
    //      "Show file paths clearly when working with files"。
    //
    // 旧实现是 nini 自己发明的两段式（"## Tool self-descriptions" +
    // "## Tool usage guidelines"），**没有**第 1、2 条——
    // 也就是说「工具集变了，模型收到的引导就变了」这件事在 nini 上不成立。
    let mut snippets: Vec<(String, String)> = Vec::new();
    let mut guidelines: Vec<String> = Vec::new();
    let mut has_bash = false;
    let mut has_powershell = false;
    let mut has_grep = false;
    let mut has_find = false;
    let mut has_ls = false;

    for tool in registry.tools() {
        let name = tool.name().to_string();
        match name.as_str() {
            "bash" => has_bash = true,
            "powershell" => has_powershell = true,
            "grep" => has_grep = true,
            "find" => has_find = true,
            "ls" => has_ls = true,
            _ => {}
        }
        if let Some(contrib) = tool.system_prompt_contribution() {
            if !contrib.snippet.trim().is_empty() {
                snippets.push((name, contrib.snippet));
            }
            for g in contrib.guidelines {
                if !g.trim().is_empty() {
                    guidelines.push(g);
                }
            }
        }
    }

    // 确定性排序（v0.8.5 的修复：HashMap 的随机顺序会打掉 LLM 的 token 缓存）
    snippets.sort_by(|a, b| a.0.cmp(&b.0));
    guidelines.sort();

    // Pi 的条件规则：有 shell、但没有 grep/find/ls → 让模型用 shell 做文件操作
    let mut all_guidelines: Vec<String> = Vec::new();
    if (has_bash || has_powershell) && !has_grep && !has_find && !has_ls {
        if has_bash && has_powershell {
            all_guidelines.push(
                "Use bash or PowerShell for file operations like listing, searching, and finding files"
                    .to_string(),
            );
        } else if has_powershell {
            all_guidelines.push(
                "Use PowerShell for file operations like listing, searching, and finding files"
                    .to_string(),
            );
        } else {
            all_guidelines.push("Use bash for file operations like ls, rg, find".to_string());
        }
    }
    all_guidelines.extend(guidelines);
    all_guidelines.push("Be concise in your responses".to_string());
    all_guidelines.push("Show file paths clearly when working with files".to_string());

    let mut out = base.unwrap_or("").to_string();
    out.push_str("\n\nAvailable tools:\n");
    if snippets.is_empty() {
        out.push_str("(none)\n");
    } else {
        for (name, snippet) in &snippets {
            out.push_str(&format!("- {name}: {snippet}\n"));
        }
    }
    out.push_str("\nGuidelines:\n");
    for g in &all_guidelines {
        out.push_str(&format!("- {g}\n"));
    }
    Some(out)
}

#[cfg(test)]
mod system_prompt_tests {
    //! v0.10.8 (T3): 这些用例的**形状**随 prompt 结构改成 Pi 的形状而变，
    //! 但**性质**不变：每个工具的 snippet/guideline 都要出现在模型收到的
    //! prompt 里，顺序确定，空的贡献不占位。
    use super::*;
    use async_trait::async_trait;

    /// Test tool with controllable contribution.
    struct ContribTool {
        name: &'static str,
        contrib: Option<ToolSystemPrompt>,
    }
    #[async_trait]
    impl Tool for ContribTool {
        fn name(&self) -> &'static str {
            self.name
        }
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: self.name.into(),
                description: "d".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }
        }
        async fn execute(&self, _a: Value, _c: ToolContext) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                content: String::new(),
                is_error: false,
                details: None,
                duration_ms: 0,
                terminate: false,
            })
        }
        fn system_prompt_contribution(&self) -> Option<ToolSystemPrompt> {
            self.contrib.clone()
        }
    }

    fn reg_with(tools: Vec<Arc<dyn Tool>>) -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        for t in tools {
            reg.register_mut(t);
        }
        reg
    }

    fn contrib(name: &'static str, snippet: &str, guidelines: &[&str]) -> Arc<dyn Tool> {
        Arc::new(ContribTool {
            name,
            contrib: Some(ToolSystemPrompt {
                snippet: snippet.into(),
                guidelines: guidelines.iter().map(|g| g.to_string()).collect(),
            }),
        })
    }

    #[test]
    fn no_tools_returns_base_unchanged() {
        let reg = reg_with(vec![]);
        let out = build_system_prompt_with_contributions(Some("base"), &reg).unwrap();
        assert!(out.starts_with("base"), "base must survive: {out}");
        // Pi 的形状下，即使没有工具也有两段（"(none)" + 固定引导）
        assert!(out.contains("Available tools:\n(none)"), "{out}");
        assert!(out.contains("- Be concise in your responses"), "{out}");
    }

    #[test]
    fn no_base_and_no_tools_returns_none() {
        let reg = reg_with(vec![]);
        // Pi 的 prompt 永远有工具段与引导段；没有基座时也要给出一份。
        // 这条断言从「返回 None」改成「仍返回一份完整的 prompt」——
        // 性质变了：以前是「没有内容就别造内容」，现在是
        // 「工具段与引导段是**必备**的，与基座无关」。
        let out = build_system_prompt_with_contributions(None, &reg).unwrap();
        assert!(out.contains("Available tools:"), "{out}");
        assert!(out.contains("- Be concise in your responses"), "{out}");
    }

    #[test]
    fn tools_with_contribution_get_appended() {
        let reg = reg_with(vec![contrib(
            "bash",
            "Run shell commands",
            &["Use absolute paths"],
        )]);
        let out = build_system_prompt_with_contributions(Some("base"), &reg).unwrap();
        assert!(out.starts_with("base"), "{out}");
        assert!(out.contains("Available tools:"), "{out}");
        assert!(out.contains("- bash: Run shell commands"), "{out}");
        assert!(out.contains("Guidelines:"), "{out}");
        assert!(out.contains("- Use absolute paths"), "{out}");
    }

    #[test]
    fn no_base_with_contribution_starts_with_header() {
        let reg = reg_with(vec![contrib("read", "Read files", &[])]);
        let out = build_system_prompt_with_contributions(None, &reg).unwrap();
        assert!(out.contains("Available tools:"), "{out}");
        assert!(out.contains("- read: Read files"), "{out}");
        // 没有工具自己的 guideline，但固定的引导仍在
        assert!(
            out.contains("- Show file paths clearly when working with files"),
            "{out}"
        );
    }

    #[test]
    fn multiple_tools_each_get_their_own_line() {
        let reg = reg_with(vec![
            contrib("bash", "run cmds", &["use paths"]),
            contrib("read", "read files", &["use limit"]),
        ]);
        let out = build_system_prompt_with_contributions(Some("base"), &reg).unwrap();
        assert!(out.contains("- bash: run cmds"), "{out}");
        assert!(out.contains("- read: read files"), "{out}");
        assert!(out.contains("- use paths"), "{out}");
        assert!(out.contains("- use limit"), "{out}");
    }

    #[test]
    fn empty_snippet_skipped() {
        let reg = reg_with(vec![contrib("x", "", &["a guideline"])]);
        let out = build_system_prompt_with_contributions(Some("base"), &reg).unwrap();
        // 空 snippet 不占位（Pi 的规则 1：没有 snippet 就不列）
        assert!(out.contains("Available tools:\n(none)"), "{out}");
        assert!(out.contains("- a guideline"), "{out}");
    }

    #[test]
    fn empty_guidelines_skipped() {
        let reg = reg_with(vec![contrib("x", "a snippet", &[])]);
        let out = build_system_prompt_with_contributions(Some("base"), &reg).unwrap();
        assert!(out.contains("- x: a snippet"), "{out}");
        // 工具自己没有 guideline，但固定的引导仍然在
        assert!(out.contains("- Be concise in your responses"), "{out}");
    }

    #[test]
    fn tool_returning_none_default_is_no_contribution() {
        struct NoneTool;
        #[async_trait]
        impl Tool for NoneTool {
            fn name(&self) -> &'static str {
                "none"
            }
            fn spec(&self) -> ToolSpec {
                ToolSpec {
                    name: "none".into(),
                    description: "d".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                }
            }
            async fn execute(&self, _a: Value, _c: ToolContext) -> Result<ToolOutput, ToolError> {
                Ok(ToolOutput {
                    content: String::new(),
                    is_error: false,
                    details: None,
                    duration_ms: 0,
                    terminate: false,
                })
            }
        }
        let reg = reg_with(vec![Arc::new(NoneTool)]);
        let out = build_system_prompt_with_contributions(Some("base"), &reg).unwrap();
        assert!(
            out.contains("Available tools:\n(none)"),
            "没有贡献的工具不进 Available tools：{out}"
        );
    }

    #[test]
    fn tool_returning_some_empty_skipped() {
        let reg = reg_with(vec![contrib("x", "", &[])]);
        let out = build_system_prompt_with_contributions(Some("base"), &reg).unwrap();
        assert!(!out.contains("- x:"), "空 snippet 不该占一行：{out}");
    }
}

/// Stub tool used by the integration test below. Defined BEFORE
/// the test module so the test can reference it via `super`.
#[cfg(test)]
mod tool_under_test {
    use super::*;
    use async_trait::async_trait;

    pub struct BashStub;
    #[async_trait]
    impl Tool for BashStub {
        fn name(&self) -> &'static str {
            "bash"
        }
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "bash".into(),
                description: String::new(),
                input_schema: serde_json::json!({}),
            }
        }
        async fn execute(&self, _a: Value, _c: ToolContext) -> Result<ToolOutput, ToolError> {
            unreachable!()
        }
        fn system_prompt_contribution(&self) -> Option<ToolSystemPrompt> {
            Some(ToolSystemPrompt {
                snippet: "test-stub-snippet".into(),
                guidelines: vec!["test-stub-guideline".into()],
            })
        }
    }
}

#[cfg(test)]
mod system_prompt_integration_tests {
    use super::*;

    /// Integration: a stub tool's contribution lands in the
    /// augmented system prompt. This is the end-to-end smoke
    /// test for Pi hook #9.
    #[test]
    fn bash_tool_contribution_lands_in_system_prompt() {
        let mut reg = ToolRegistry::new();
        reg.register_mut(Arc::new(tool_under_test::BashStub));
        let out = build_system_prompt_with_contributions(Some("BASE"), &reg).unwrap();
        assert!(out.starts_with("BASE"));
        // The stub tool provides a recognizable snippet.
        assert!(
            out.contains("test-stub-snippet"),
            "expected stub snippet in: {out}"
        );
        assert!(out.contains("test-stub-guideline"));
    }
}
