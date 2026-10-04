//! `bash` tool: shell execution with timeout, kill-process-tree, and tail-truncation.
//!
//! Mirrors spec `packages/coding-agent/src/core/tools/bash.ts`. Key behaviors:
//! - Spawn the child in its own process group so we can kill the entire tree
//!   on timeout (`setsid(2)` on Unix; on Windows we fall back to per-PID kill).
//! - On timeout: send SIGTERM, wait up to `grace_ms` for graceful exit, then SIGKILL.
//! - Truncate output if it exceeds `max_output_bytes` (default 1 MiB) or
//!   `max_output_lines` (default 2000): keep first half + ... + last half.
//!
//! **v0.8.9 (P0-1)**: the child is now actually put in its own process
//! group. It never was — the `pre_exec(setsid)` call that made
//! `killpg(child_pid, ..)` meaningful lived only in `bash_runner.rs`
//! (the hook path), while the LLM's `bash` tool call went through this
//! file. See `crate::procgroup` for the full write-up and the
//! reproduction.
#![allow(dead_code)] // Forward-compat fields for future features

use async_trait::async_trait;
use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolSpec, ToolSystemPrompt};
use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::time::timeout;

/// Default command timeout in seconds (2 minutes, matching spec).
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Grace period between SIGTERM and SIGKILL.
///
/// v0.8.9 (P0-1): was 5000 ms. That number was tuned to cover a
/// process-group kill that never actually fired (see the module docs
/// in `crate::procgroup`), so every timeout cost 1 s + 5 s of dead
/// waiting — measured 6177 ms for a `timeout: 1` call. With
/// `setsid(2)` in place SIGTERM reaches the whole group immediately,
/// and anything still alive after 500 ms is not going to cooperate.
pub const KILL_GRACE_MS: u64 = 500;

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
#[allow(dead_code)]
struct BashDetails {
    exit_code: Option<i32>,
    signal: Option<String>,
    duration_ms: u64,
    truncated: bool,
    /// What the command *actually* printed, not what we kept. Can
    /// exceed the capture cap.
    original_bytes: usize,
    original_lines: usize,
    /// v0.8.9 (P0-2): true when a stream hit [`STREAM_CAPTURE_CAP`] and
    /// stopped accumulating (while still counting). Tells the caller the
    /// content is a prefix, not the whole thing.
    stream_capped: bool,
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
            description: "Run a shell command (ls, grep, cat, mkdir, cargo test, etc.) and \
                          return its stdout + stderr + exit code. Set `timeout` to bound \
                          long-running commands; on timeout the process tree is killed."
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
        //
        // v0.8.9 (P0-1): `shell_command` is the single place that puts
        // the child in its own process group. It used to be duplicated
        // in `bash_runner.rs` — and this file's copy was missing the
        // `pre_exec(setsid)` entirely, which silently turned the
        // process-group kill into a no-op on the LLM's bash path.
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut cmd = crate::procgroup::shell_command_tokio(&shell, &parsed.command, &ctx.cwd);

        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::Io(format!("failed to spawn shell: {e}")))?;

        let pid = child
            .id()
            .ok_or_else(|| ToolError::Internal("no child pid".to_string()))?;

        // Capture output concurrently with timeout.
        //
        // v0.8.9 (P0-2): both readers used to be unbounded
        // `read_to_end`. A `yes` / `find /` / noisy build would fill
        // RAM until the process died, and only then would `truncate`
        // run and throw 99.99% of it away. Now each stream keeps at
        // most `capture_cap` bytes while still counting everything it
        // saw, and keeps draining the pipe past the cap so the child
        // never blocks on a full buffer.
        let capture_cap = ctx.max_output_bytes.max(64 * 1024).min(STREAM_CAPTURE_CAP);
        let mut stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let stdout_cap = capture_cap;
        let stdout_task =
            tokio::spawn(async move { read_stream_capped(&mut stdout, stdout_cap).await });
        let stderr_cap = capture_cap;
        let stderr_task =
            tokio::spawn(async move { read_stream_capped(&mut stderr, stderr_cap).await });

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

        // Join output tasks. Each returns (kept, total_bytes,
        // total_lines, capped) — the totals are what the stream really
        // carried, which can exceed `kept.len()` when the capture cap
        // kicked in.
        let (stdout_bytes, stdout_total_b, stdout_total_l, stdout_capped) = stdout_task
            .await
            .map_err(|e| ToolError::Internal(e.to_string()))?;
        let (stderr_bytes, stderr_total_b, stderr_total_l, stderr_capped) = stderr_task
            .await
            .map_err(|e| ToolError::Internal(e.to_string()))?;

        // Compose output: stdout + stderr (stderr after stdout for clarity)
        let mut raw = stdout_bytes;
        let has_stderr = !stderr_bytes.is_empty();
        if has_stderr {
            if !raw.is_empty() && !raw.ends_with(b"\n") {
                raw.push(b'\n');
            }
            raw.extend_from_slice(b"[stderr]\n");
            raw.extend_from_slice(&stderr_bytes);
        }

        // Truncate. The `[stderr]\n` separator contributes bytes and a
        // line to the composed stream, so fold them into the totals
        // rather than letting the markers under-report.
        let total_bytes = stdout_total_b + stderr_total_b + usize::from(has_stderr);
        let total_lines = stdout_total_l + stderr_total_l + usize::from(has_stderr);
        let stream_capped = stdout_capped || stderr_capped;
        let (truncated_content, truncated) = truncate(
            &raw,
            ctx.max_output_bytes,
            ctx.max_output_lines,
            total_bytes,
            total_lines,
            stream_capped,
        );

        let duration_ms = started.elapsed().as_millis() as u64;

        let details = BashDetails {
            exit_code,
            signal: signal_name,
            duration_ms,
            truncated,
            original_bytes: total_bytes,
            original_lines: total_lines,
            stream_capped,
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
            duration_ms,
        })
    }

    fn system_prompt_contribution(&self) -> Option<ToolSystemPrompt> {
        Some(ToolSystemPrompt {
            snippet: "Run shell commands (ls, grep, cat, cargo test, ...).".into(),
            guidelines: vec![
                "Default for any shell operation; pick this unless a more \
                 specific tool (read, grep, find, edit) clearly fits."
                    .into(),
                "Set `timeout` for anything that could run unboundedly.".into(),
                "Confirm with the user before destructive commands \
                 (`rm -rf`, `git push --force`)."
                    .into(),
            ],
        })
    }
}

/// Kill the entire process tree rooted at `pid`. Unix-only.
#[cfg(unix)]
pub(crate) fn kill_process_tree(pid: u32) {
    // First, try to kill the process group. Reject pids that don't fit in
    // i32 — calling killpg with a wrapped negative value would target the
    // current process group, killing nini itself.
    let pgid = match i32::try_from(pid) {
        Ok(p) => Pid::from_raw(p),
        Err(_) => {
            eprintln!("[nini] pid {pid} out of i32 range; skipping killpg");
            return;
        }
    };
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

/// Largest byte count we will ever hold in memory for one stream.
///
/// The *content* budget is `ToolContext::max_output_bytes` (1 MiB by
/// default); this is a hard ceiling on the capture buffer so a runaway
/// command (`yes`, `find /`, a noisy build) cannot exhaust memory before
/// truncation gets a chance to run.
///
/// v0.8.9 (P0-2): the reader used to call `read_to_end` with no bound,
/// so the "1 MiB budget" was a purely cosmetic post-hoc number.
pub const STREAM_CAPTURE_CAP: usize = 8 * 1024 * 1024;

/// Truncate output to fit within `max_bytes` and `max_lines`. Keeps the first
/// and last halves equally, separated by a marker.
///
/// `total_bytes` / `total_lines` are what the stream *actually* carried
/// (measured while reading, which may exceed `raw.len()` when the reader
/// hit [`STREAM_CAPTURE_CAP`]). `capped` says whether it did.
///
/// **v0.8.9 (P0-3)**: the byte-level cut used to be a raw `&kept[..half]`
/// on a `String`. Any multi-byte output over the byte budget panicked:
///
/// ```text
/// panicked at src/bash.rs: end byte index 524288 is not a char
/// boundary; it is inside '文' (bytes 524286..524289 of string)
/// ```
///
/// This is the same class of bug fixed in v0.8.4 for `render_tool_call`
/// and `render_bash_execution` — `truncate` was simply missed both times,
/// and unlike those two it lives on the tool-execution path, so a large
/// CJK `bash` result took the whole agent down. Every slice below now goes
/// through [`floor_char_boundary`].
///
/// The function is also single-pass now: it used to `lines().collect()`
/// the entire output into a `Vec<&str>` just to take two halves, which
/// for a 6-million-line capture is ~50 MB of pointers on its own.
fn truncate(
    raw: &[u8],
    max_bytes: usize,
    max_lines: usize,
    total_bytes: usize,
    total_lines: usize,
    capped: bool,
) -> (String, bool) {
    let truncated = capped || total_bytes > max_bytes || total_lines > max_lines;

    // Fast path: nothing was dropped.
    if !truncated {
        return (String::from_utf8_lossy(raw).into_owned(), false);
    }

    let initial = String::from_utf8_lossy(raw).into_owned();
    let mut kept = if total_lines > max_lines {
        // Single pass: keep the head, and a bounded rolling tail.
        // `max_lines` can be 0 or 1 — clamp so the arithmetic below
        // never underflows.
        let half = (max_lines / 2).max(1);
        let mut head: Vec<&str> = Vec::with_capacity(half.min(initial.len()));
        let mut tail: std::collections::VecDeque<&str> = std::collections::VecDeque::new();
        for line in initial.lines() {
            if head.len() < half {
                head.push(line);
            } else {
                if tail.len() == half {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        }
        let omitted = total_lines.saturating_sub(head.len() + tail.len());
        let mut s = head.join("\n");
        s.push_str(&format!("\n[... {omitted} lines omitted ...]\n"));
        s.push_str(&tail.iter().copied().collect::<Vec<_>>().join("\n"));
        s
    } else {
        initial
    };

    // If still too large by bytes, hard-truncate with a tail marker.
    if kept.len() > max_bytes {
        let half = (max_bytes / 2).max(1);
        let head_end = floor_char_boundary(&kept, half);
        let tail_start = floor_char_boundary(&kept, kept.len().saturating_sub(half));
        // `if kept.len() > max_bytes` guarantees `head_end < tail_start`
        // for any budget of at least 2, so the two slices never overlap.
        let kept_bytes = head_end + (kept.len() - tail_start);
        let mut out = String::with_capacity(max_bytes + 128);
        out.push_str(&kept[..head_end]);
        out.push_str(&format!(
            "\n[... {} bytes omitted ...]\n",
            total_bytes.saturating_sub(kept_bytes)
        ));
        out.push_str(&kept[tail_start..]);
        kept = out;
    }

    (kept, true)
}

/// Largest byte index <= `at` that is a UTF-8 character boundary in `s`.
///
/// Same contract as `nini_tui::width::floor_char_boundary` (which is why
/// that one exists — v0.8.4 added it after `render_tool_call` panicked on
/// CJK args). Duplicated rather than shared because `nini-tui` depends on
/// `nini-tools`, not the other way round; if this ever grows a third
/// copy, promote it to `nini-core`.
#[inline]
fn floor_char_boundary(s: &str, at: usize) -> usize {
    if at >= s.len() {
        return s.len();
    }
    let mut i = at;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Read `r` to EOF, keeping at most `cap` bytes in memory while still
/// counting every byte and line that went past.
///
/// Stopping *capture* is not the same as stopping *reading*: once the
/// cap is hit we keep draining the pipe (discarding) so the child never
/// blocks on a full pipe and never has to be killed for being chatty.
/// The returned totals stay honest, which is what the `…N bytes
/// omitted` markers report.
async fn read_stream_capped<R>(r: &mut R, cap: usize) -> (Vec<u8>, usize, usize, bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut kept: Vec<u8> = Vec::with_capacity(8192.min(cap));
    let mut chunk = vec![0u8; 64 * 1024];
    let mut total_bytes = 0usize;
    let mut total_lines = 0usize;
    let mut last_byte: Option<u8> = None;

    loop {
        let n = match r.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        total_bytes += n;
        total_lines += chunk[..n].iter().filter(|&&c| c == b'\n').count();
        last_byte = Some(chunk[n - 1]);
        if kept.len() < cap {
            let take = (cap - kept.len()).min(n);
            kept.extend_from_slice(&chunk[..take]);
        }
    }

    // Mirror `bytecount_lines`: a trailing partial line still counts.
    if total_bytes > 0 && last_byte != Some(b'\n') {
        total_lines += 1;
    }
    let capped = total_bytes > kept.len();

    // Don't hand `from_utf8_lossy` a chunk we cut mid-character — it
    // would render as U+FFFD instead of truncating cleanly.
    if capped {
        let mut cut = kept.len();
        while cut > 0 && !is_utf8_boundary(&kept[..cut]) {
            cut -= 1;
        }
        kept.truncate(cut);
    }

    (kept, total_bytes, total_lines, capped)
}

/// Is `bytes` a valid UTF-8 prefix boundary? We can't use `str` here
/// because the buffer may not be valid UTF-8 at all, so walk back over
/// continuation bytes (0b10xxxxxx) to find a lead byte.
fn is_utf8_boundary(bytes: &[u8]) -> bool {
    match bytes.last() {
        None => true,
        Some(&b) => (b & 0b1100_0000) != 0b1000_0000,
    }
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

    #[test]
    fn bash_spec_is_concise_and_nonoverlapping() {
        // v0.8 regression guard: snippet + description must not duplicate
        // each other, and snippet must be short (Pi-style: 4–7 words).
        let tool = BashTool::new();
        let contrib = tool.system_prompt_contribution().unwrap();
        assert!(
            contrib.snippet.split_whitespace().count() <= 10,
            "bash snippet too long: {} words — {}",
            contrib.snippet.split_whitespace().count(),
            contrib.snippet
        );
        let spec = tool.spec();
        assert!(
            !spec.description.contains("via $SHELL -c"),
            "bash description should not duplicate the snippet's mechanics"
        );
    }

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

    // ---- v0.8.9 (P0-1) ----------------------------------------------
    // `timeout_kills_slow_command` above passed while the process-group
    // kill was dead code, because the child eventually died via
    // `kill_on_drop`. The two tests below check the parts that actually
    // regressed: the wall-clock cost of a timeout, and whether the kill
    // reaches past the direct child.

    /// Scan /proc for a live process whose cmdline contains `needle`.
    /// Pure std so the test doesn't depend on `pgrep`/`ps` being
    /// installed (CI runs on macOS too, where /proc doesn't exist —
    /// hence the `cfg(target_os = "linux")` on the callers).
    #[cfg(target_os = "linux")]
    fn pids_with_cmdline_containing(needle: &str) -> Vec<u32> {
        let mut hits = Vec::new();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return hits;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Ok(raw) = std::fs::read(entry.path().join("cmdline")) else {
                continue;
            };
            let cmdline = String::from_utf8_lossy(&raw);
            if cmdline.contains(needle) {
                if let Ok(pid) = name.parse::<u32>() {
                    hits.push(pid);
                }
            }
        }
        hits
    }

    #[cfg(target_os = "linux")]
    fn kill_pids(pids: &[u32]) {
        for pid in pids {
            let _ = std::process::Command::new("kill")
                .arg("-9")
                .arg(pid.to_string())
                .status();
        }
    }

    /// A timeout used to cost `timeout + KILL_GRACE_MS` unconditionally,
    /// because `killpg` targeted a process group that didn't exist and
    /// the shell sat out the whole grace period before `kill_on_drop`
    /// collected it. Measured 6177 ms for `timeout: 1`.
    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn timeout_does_not_sit_out_the_full_grace_period() {
        let tool = BashTool::new();
        let started = std::time::Instant::now();
        let out = tool
            .execute(
                json!({"command": "sleep 30", "timeout": 1}),
                ToolContext::default(),
            )
            .await
            .unwrap();
        let elapsed = started.elapsed();

        assert!(out.is_error);
        let details: BashDetails = serde_json::from_value(out.details.unwrap()).unwrap();
        assert_eq!(details.signal.as_deref(), Some("timeout"));

        // 1 s timeout + 500 ms grace + slack. The old behaviour needed
        // 1 s + 5000 ms, so this bound fails loudly on a regression.
        assert!(
            elapsed < Duration::from_millis(2500),
            "timeout took {elapsed:?} — SIGTERM is probably not reaching the \
             process group again (budget 1s timeout + {}ms grace)",
            KILL_GRACE_MS
        );
    }

    /// The process-group kill exists so a timeout reaches *descendants*,
    /// not just the shell we spawned. Before v0.8.9 only the `sysinfo`
    /// descendant walk did any work; the group kill was a no-op, and
    /// anything that escaped the parent chain (or raced the walk) lived
    /// on. This test uses a grandchild to exercise the group path.
    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn timeout_kills_grandchildren_via_process_group() {
        // A marker unlikely to collide with anything else on the box.
        const MARKER: &str = "nini-p0-1-marker-4190773";
        let tool = BashTool::new();
        let command = format!("sh -c 'sleep 400 # {MARKER}' & wait");
        let out = tool
            .execute(
                json!({"command": command, "timeout": 1}),
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(out.is_error);

        // Give the kernel a beat to reap the group.
        tokio::time::sleep(Duration::from_millis(400)).await;
        let survivors = pids_with_cmdline_containing(MARKER);
        kill_pids(&survivors);

        assert!(
            survivors.is_empty(),
            "timeout left {} grandchild process(es) alive (pids {survivors:?}) — \
             the process group was not killed",
            survivors.len()
        );
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
        let raw = b"hello\nworld\n";
        let (out, truncated) = truncate(raw, 1024, 100, raw.len(), bytecount_lines(raw), false);
        assert_eq!(out, "hello\nworld\n");
        assert!(!truncated);
    }

    #[test]
    fn truncate_omits_middle_when_too_many_lines() {
        let mut input = String::new();
        for i in 0..1000 {
            input.push_str(&format!("line {i}\n"));
        }
        let (out, truncated) = truncate(
            input.as_bytes(),
            1024 * 1024,
            100,
            input.len(),
            bytecount_lines(input.as_bytes()),
            false,
        );
        assert!(truncated);
        assert!(out.contains("omitted"));
        assert!(out.contains("line 0"));
        assert!(out.contains("line 999"));
        assert!(!out.contains("line 500"));
    }

    // ---- v0.8.9 (P0-3) regression guards ------------------------------
    // `truncate` used to cut the byte budget with a raw `&kept[..half]`.
    // On any multi-byte output over the budget that lands inside a
    // character and panics:
    //
    //     panicked at src/bash.rs: end byte index 524288 is not a char
    //     boundary; it is inside '文' (bytes 524286..524289 of string)
    //
    // Same class as the v0.8.4 `render_tool_call` / `render_bash_execution`
    // panics, except this one is on the tool-execution path — a >1 MiB
    // CJK bash result unwound the entire agent, and none of the 812
    // tests caught it because every fixture was ASCII.

    #[test]
    fn truncate_multibyte_output_does_not_panic() {
        // 3 MiB of 3-byte CJK against a 1 MiB budget, i.e. the exact
        // shape that panicked before.
        let cjk = "文";
        let count = (3 * 1024 * 1024) / cjk.len();
        let input: String = std::iter::repeat_n(cjk, count).collect();
        let raw = input.as_bytes();
        let (out, truncated) = truncate(
            raw,
            1024 * 1024,
            2000,
            raw.len(),
            bytecount_lines(raw),
            false,
        );
        assert!(truncated);
        assert!(
            out.contains("bytes omitted"),
            "got: {}",
            &out[..out.len().min(200)]
        );
        // Must be valid UTF-8 end to end — `truncate` returns a String
        // so this is really a "did we cut mid-char" assertion.
        assert!(out.len() > 0);
    }

    #[test]
    fn truncate_multibyte_output_at_every_byte_budget_does_not_panic() {
        // Walk the budget across a range that lands `half` on both sides
        // of every character boundary — a single size would only prove
        // that one size is lucky.
        let input = "文".repeat(5000);
        for budget in 1..600usize {
            let raw = input.as_bytes();
            let (_out, truncated) =
                truncate(raw, budget, 2000, raw.len(), bytecount_lines(raw), false);
            if budget < raw.len() {
                assert!(truncated, "budget {budget} should have truncated");
            }
        }
    }

    #[test]
    fn floor_char_boundary_lands_on_boundaries() {
        let s = "a文b";
        assert_eq!(floor_char_boundary(s, 0), 0);
        assert_eq!(floor_char_boundary(s, 1), 1); // 'a'
        assert_eq!(floor_char_boundary(s, 2), 1); // inside '文' → back to 1
        assert_eq!(floor_char_boundary(s, 4), 4); // 'b'
        assert_eq!(floor_char_boundary(s, 99), s.len());
    }

    // ---- v0.8.9 (P0-2) regression guards ------------------------------

    #[tokio::test]
    async fn read_stream_capped_bounds_memory_but_keeps_counting() {
        // 4 MiB through a 64 KiB cap: the buffer must stay tiny while
        // the reported totals stay honest.
        let data = vec![b'x'; 4 * 1024 * 1024];
        let mut cursor = std::io::Cursor::new(data.clone());
        let (kept, total_bytes, _lines, capped) = read_stream_capped(&mut cursor, 64 * 1024).await;

        assert!(capped, "4 MiB through a 64 KiB cap must report capped");
        assert!(
            kept.len() <= 64 * 1024,
            "kept {} bytes, cap was 64 KiB",
            kept.len()
        );
        assert_eq!(
            total_bytes,
            data.len(),
            "total must reflect the real stream"
        );
    }

    #[tokio::test]
    async fn read_stream_capped_drains_past_the_cap() {
        // The reader must keep consuming after the cap so the child
        // never blocks on a full pipe. A 1 MiB stream through a 4 KiB cap
        // completing at all is the evidence.
        let data = vec![b'y'; 1024 * 1024];
        let mut cursor = std::io::Cursor::new(data.clone());
        let (kept, total_bytes, _lines, capped) = read_stream_capped(&mut cursor, 4096).await;
        assert!(capped);
        assert_eq!(kept.len(), 4096);
        assert_eq!(total_bytes, 1024 * 1024);
    }

    #[tokio::test]
    async fn read_stream_capped_counts_lines_like_bytecount_lines() {
        for input in [
            "".to_string(),
            "a".to_string(),
            "a\n".to_string(),
            "a\nb".to_string(),
            "a\nb\n".to_string(),
            "\n\n\n".to_string(),
        ] {
            let mut cursor = std::io::Cursor::new(input.clone().into_bytes());
            let (_kept, _bytes, lines, _capped) =
                read_stream_capped(&mut cursor, 1024 * 1024).await;
            assert_eq!(
                lines,
                bytecount_lines(input.as_bytes()),
                "line count mismatch for {input:?}"
            );
        }
    }

    #[tokio::test]
    async fn huge_command_output_is_capped_not_buffered() {
        // End-to-end: a 64 MB single-line stream with the default
        // 64 KiB-ish floor. Before v0.8.9 this came back reporting
        // original_bytes=67108864 while holding all of it in RAM.
        let tool = BashTool::new();
        let mut ctx = ToolContext::default();
        ctx.max_output_bytes = 128 * 1024;
        let out = tool
            .execute(
                json!({"command": "yes ABCDEFGHIJKLMNOP | head -c 67108864", "timeout": 60}),
                ctx,
            )
            .await
            .unwrap();
        let details: BashDetails = serde_json::from_value(out.details.unwrap()).unwrap();

        assert!(details.truncated);
        assert!(details.stream_capped, "64 MB should have hit the cap");
        assert_eq!(
            details.original_bytes, 67_108_864,
            "the reported total must be the real stream size, not the kept size"
        );
        // The whole point: the content we hand back is budgeted, not the
        // whole stream.
        assert!(
            out.content.len() < 512 * 1024,
            "returned {} bytes for a 128 KiB budget",
            out.content.len()
        );
    }

    #[test]
    fn line_counter_handles_trailing_newline() {
        assert_eq!(bytecount_lines(b"a\nb\nc\n"), 3);
        assert_eq!(bytecount_lines(b"a\nb\nc"), 3);
        assert_eq!(bytecount_lines(b""), 0);
        assert_eq!(bytecount_lines(b"\n"), 1);
    }

    #[cfg(unix)]
    #[test]
    fn kill_process_tree_skips_pids_outside_i32_range() {
        // A pid that doesn't fit in i32 (e.g. u32::MAX) used to wrap to
        // a negative number, which Pid::from_raw(negative) would interpret
        // as a different process group — potentially the current one,
        // killing nini itself. The fix short-circuits.
        super::kill_process_tree(u32::MAX);
        // The test passes if we got here without panicking or killing
        // ourselves. (No explicit assertion — absence of crash is the
        // verification.)
    }
}
