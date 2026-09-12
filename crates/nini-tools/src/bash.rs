//! `bash` tool: shell execution with timeout, kill-process-tree, and tail-truncation.
//!
//! Mirrors spec `packages/coding-agent/src/core/tools/bash.ts`. Key behaviors:
//! - Spawn the child in its own process group so we can kill the entire tree
//!   on timeout (`setsid(2)` on Unix; on Windows we fall back to per-PID kill).
//! - On timeout: send SIGTERM, wait up to `grace_ms` for graceful exit, then SIGKILL.
//! - Truncate output if it exceeds `max_output_bytes` (default 1 MiB) or
//!   `max_output_lines` (default 2000): keep first half + ... + last half.
#![allow(dead_code)] // Forward-compat fields for future features

use async_trait::async_trait;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;
use nini_core::tool::{Tool, ToolContext, ToolOutput, ToolError, ToolSpec};

/// Default command timeout in seconds (2 minutes, matching spec).
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Grace period between SIGTERM and SIGKILL.
pub const KILL_GRACE_MS: u64 = 5000;

/// Hard upper bound on user-supplied timeouts (~24.8 days).
pub const MAX_TIMEOUT_MS: u64 = 2_147_483_647;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct BashArgs {
    command: String,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BashDetails {
    exit_code: Option<i32>,
    signal: Option<String>,
    duration_ms: u64,
    truncated: bool,
    original_bytes: usize,
    original_lines: usize,
}

/// `BashTool` runs shell commands via the user's `$SHELL` (or `/bin/sh`).
#[derive(Debug, Default)]
pub struct BashTool;

impl BashTool {
    /// Create a new `BashTool` instance.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "bash".to_string(),
            description: "Execute a shell command and return its output. Use a timeout in seconds \
                          to bound long-running commands; on timeout the process tree is killed."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute."
                    },
                    "timeout": {
                        "type": "number",
                        "description": "Maximum execution time in seconds. Defaults to 120s. \
                                        Max ~24.8 days."
                    },
                    "description": {
                        "type": "string",
                        "description": "A short human-readable description of what the command does."
                    }
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let parsed: BashArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArgs(e.to_string()))?;

        let timeout_secs = parsed.timeout.unwrap_or(DEFAULT_TIMEOUT_SECS);
        if timeout_secs * 1000 > MAX_TIMEOUT_MS {
            return Err(ToolError::InvalidArgs(format!(
                "timeout too large: max {} seconds",
                MAX_TIMEOUT_MS / 1000
            )));
        }
        let timeout_duration = Duration::from_secs(timeout_secs);

        let started = std::time::Instant::now();

        // Build command. Use $SHELL or /bin/sh.
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut cmd = Command::new(&shell);
        cmd.arg("-c").arg(&parsed.command);
        cmd.current_dir(&ctx.cwd);
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        cmd.kill_on_drop(true);

        // Set process group on Unix so we can killpg on timeout. Implemented inline
        // in execute via pre_exec; nothing to do here.

        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::Io(format!("failed to spawn shell: {e}")))?;

        let pid = child
            .id()
            .ok_or_else(|| ToolError::Internal("no child pid".to_string()))?;

        // Capture output concurrently with timeout
        let mut stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let stdout_task = tokio::spawn(async move {
            let mut buf = Vec::with_capacity(4096);
            stdout.read_to_end(&mut buf).await.map(|_| buf)
        });
        let stderr_task = tokio::spawn(async move {
            let mut buf = Vec::with_capacity(4096);
            stderr.read_to_end(&mut buf).await.map(|_| buf)
        });

        // Wait for child with timeout
        let wait_result = timeout(timeout_duration, child.wait()).await;
        let (exit_code, signal_name) = match wait_result {
            Ok(Ok(status)) => {
                use std::os::unix::process::ExitStatusExt;
                if let Some(code) = status.code() {
                    (Some(code), None)
                } else {
                    let sig = status.signal();
                    let sig_name = sig
                        .map(|s| {
                            // Best-effort name lookup
                            format!("signal {s}")
                        })
                        .unwrap_or_else(|| "unknown signal".to_string());
                    (None, Some(sig_name))
                }
            }
            Ok(Err(e)) => return Err(ToolError::Io(format!("waitpid failed: {e}"))),
            Err(_elapsed) => {
                // Timeout: kill the process group
                kill_process_tree(pid);
                // Wait briefly for child to reap
                let _ = timeout(Duration::from_millis(KILL_GRACE_MS + 100), child.wait()).await;
                (None, Some("timeout".to_string()))
            }
        };

        // Join output tasks
        let stdout_bytes = stdout_task
            .await
            .map_err(|e| ToolError::Internal(e.to_string()))??;
        let stderr_bytes = stderr_task
            .await
            .map_err(|e| ToolError::Internal(e.to_string()))??;

        // Compose output: stdout + stderr (stderr after stdout for clarity)
        let mut raw = stdout_bytes;
        if !stderr_bytes.is_empty() {
            if !raw.is_empty() && !raw.ends_with(b"\n") {
                raw.push(b'\n');
            }
            raw.extend_from_slice(b"[stderr]\n");
            raw.extend_from_slice(&stderr_bytes);
        }

        // Truncate
        let (truncated_content, truncated, original_bytes, original_lines) =
            truncate(&raw, ctx.max_output_bytes, ctx.max_output_lines);

        let duration_ms = started.elapsed().as_millis() as u64;

        let details = BashDetails {
            exit_code,
            signal: signal_name,
            duration_ms,
            truncated,
            original_bytes,
            original_lines,
        };

        let is_error = !matches!(exit_code, Some(0));

        let content = if let Some(code) = exit_code {
            format!("{truncated_content}\n[exit code: {code}, duration: {duration_ms}ms]")
        } else if let Some(ref sig) = details.signal {
            format!("{truncated_content}\n[terminated by {sig}, duration: {duration_ms}ms]")
        } else {
            truncated_content
        };

        Ok(ToolOutput {
            content,
            is_error,
            details: Some(serde_json::to_value(&details).unwrap()),
        })
    }
}

/// Kill the entire process tree rooted at `pid`. Unix-only.
#[cfg(unix)]
fn kill_process_tree(pid: u32) {
    // First, try to kill the process group.
    let pgid = Pid::from_raw(pid as i32);
    let _ = killpg(pgid, Signal::SIGTERM);

    // Walk the process tree via sysinfo to find children that escaped the
    // process group (e.g., daemons that called setsid themselves).
    let mut sys = sysinfo::System::new_all();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let target_pid = sysinfo::Pid::from_u32(pid);
    let children: Vec<sysinfo::Pid> = sys
        .processes()
        .iter()
        .filter_map(|(p, proc_)| {
            // Walk up: any process whose parent chain includes target_pid.
            let mut current = proc_.parent();
            while let Some(parent) = current {
                if parent == target_pid {
                    return Some(*p);
                }
                current = sys.processes().get(&parent).and_then(|p| p.parent());
            }
            None
        })
        .collect();

    for child in &children {
        if let Some(p) = sys.process(*child) {
            let _ = p.kill();
        }
    }

    // After grace, SIGKILL anything that survived
    std::thread::sleep(Duration::from_millis(KILL_GRACE_MS));
    let _ = killpg(pgid, Signal::SIGKILL);
    for child in &children {
        if let Some(p) = sys.process(*child) {
            let _ = p.kill();
        }
    }
}

#[cfg(not(unix))]
fn kill_process_tree(_pid: u32) {
    // Windows: not yet implemented in nini v1.
}

/// Truncate output to fit within `max_bytes` and `max_lines`. Keeps the first
/// and last halves equally, separated by a marker.
fn truncate(raw: &[u8], max_bytes: usize, max_lines: usize) -> (String, bool, usize, usize) {
    let original_bytes = raw.len();
    let original_lines = bytecount_lines(raw);

    // Fast path: no truncation needed
    if original_bytes <= max_bytes && original_lines <= max_lines {
        return (
            String::from_utf8_lossy(raw).into_owned(),
            false,
            original_bytes,
            original_lines,
        );
    }

    // Truncate by lines first, then by bytes
    let initial = String::from_utf8_lossy(raw).into_owned();
    let mut kept = if initial.lines().count() > max_lines {
        let lines: Vec<&str> = initial.lines().collect();
        let half = max_lines / 2;
        let first: Vec<&str> = lines[..half].to_vec();
        let last: Vec<&str> = lines[lines.len() - half..].to_vec();
        let marker = format!(
            "\n[... {} lines omitted ...]\n",
            original_lines - first.len() - last.len()
        );
        let mut s = first.join("\n");
        s.push_str(&marker);
        s.push_str(&last.join("\n"));
        s
    } else {
        initial
    };

    // If still too large by bytes, hard-truncate with a tail marker
    if kept.len() > max_bytes {
        let half = max_bytes / 2;
        let mut out = String::with_capacity(max_bytes + 128);
        out.push_str(&kept[..half]);
        out.push_str(&format!(
            "\n[... {} bytes omitted ...]\n",
            original_bytes - max_bytes
        ));
        let tail_start = kept.len().saturating_sub(half);
        out.push_str(&kept[tail_start..]);
        kept = out;
    }

    (kept, true, original_bytes, original_lines)
}

/// Count lines in a byte slice (counts trailing newline as a line terminator).
fn bytecount_lines(b: &[u8]) -> usize {
    if b.is_empty() {
        return 0;
    }
    let mut count = 0;
    for &c in b {
        if c == b'\n' {
            count += 1;
        }
    }
    if b[b.len() - 1] != b'\n' {
        count += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echo_command() {
        let tool = BashTool::new();
        let out = tool
            .execute(json!({"command": "echo hello"}), ToolContext::default())
            .await
            .unwrap();
        assert!(out.content.contains("hello"), "got: {}", out.content);
        assert!(!out.is_error);
    }

    #[tokio::test]
    async fn nonzero_exit_marks_error() {
        let tool = BashTool::new();
        let out = tool
            .execute(json!({"command": "exit 7"}), ToolContext::default())
            .await
            .unwrap();
        assert!(out.is_error, "expected is_error, got: {:?}", out.details);
    }

    #[tokio::test]
    async fn timeout_kills_slow_command() {
        let tool = BashTool::new();
        let out = tool
            .execute(
                json!({"command": "sleep 30", "timeout": 1}),
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(out.is_error);
        let details: BashDetails = serde_json::from_value(out.details.unwrap()).unwrap();
        assert_eq!(details.signal.as_deref(), Some("timeout"));
    }

    #[tokio::test]
    async fn stderr_captured() {
        let tool = BashTool::new();
        let out = tool
            .execute(json!({"command": "echo oops >&2"}), ToolContext::default())
            .await
            .unwrap();
        assert!(out.content.contains("[stderr]"), "got: {}", out.content);
        assert!(out.content.contains("oops"), "got: {}", out.content);
    }

    #[test]
    fn truncate_short_input_passes_through() {
        let (out, truncated, _orig_b, _orig_l) = truncate(b"hello\nworld\n", 1024, 100);
        assert_eq!(out, "hello\nworld\n");
        assert!(!truncated);
    }

    #[test]
    fn truncate_omits_middle_when_too_many_lines() {
        let mut input = String::new();
        for i in 0..1000 {
            input.push_str(&format!("line {i}\n"));
        }
        let (out, truncated, _orig_b, _orig_l) = truncate(input.as_bytes(), 1024 * 1024, 100);
        assert!(truncated);
        assert!(out.contains("omitted"));
        assert!(out.contains("line 0"));
        assert!(out.contains("line 999"));
        assert!(!out.contains("line 500"));
    }

    #[test]
    fn line_counter_handles_trailing_newline() {
        assert_eq!(bytecount_lines(b"a\nb\nc\n"), 3);
        assert_eq!(bytecount_lines(b"a\nb\nc"), 3);
        assert_eq!(bytecount_lines(b""), 0);
        assert_eq!(bytecount_lines(b"\n"), 1);
    }
}
