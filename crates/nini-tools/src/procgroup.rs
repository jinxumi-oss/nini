//! Process-group isolation for spawned shells.
//!
//! **v0.8.9 (P0-1)**: nini had two independent bash implementations —
//! `BashTool::execute` (`bash.rs`) and `BashRunner` (`bash_runner.rs`) —
//! and only the second one called `setsid(2)`. The first one carried a
//! comment claiming the `pre_exec` hook was "implemented inline in
//! execute", but there was no `pre_exec` anywhere in the file.
//!
//! Consequence, measured on the `BashTool` path (the one the LLM
//! actually uses):
//!
//! ```text
//! child pid=550789
//! child's real pgid (read /proc/550789/stat field 5) = 550786   ← the parent's group
//! killpg(child_pid=550789) target group exists? false          ← ESRCH
//! ```
//!
//! `kill_process_tree` then passed `child.id()` to `killpg()` as if it
//! were a PGID, hit a non-existent group, and swallowed the `ESRCH`.
//! The process-group kill was dead code; only the `sysinfo` descendant
//! walk did anything, and a `timeout: 1` call really cost 6177 ms
//! (1 s timeout + the full 5 s grace, after which the child was finally
//! reaped by `kill_on_drop`).
//!
//! Both paths now go through this module so they cannot drift apart
//! again.

use std::process::{Command, Stdio};

// `setsid(2)` wrapper. The libc dep is isolated here so non-Unix
// targets keep building untouched.
//
// We use a `//` comment instead of `///` because rustdoc does not
// generate documentation for `extern` blocks, so a doc comment would
// be flagged as `unused_doc_comments`.
#[cfg(unix)]
unsafe extern "C" {
    fn setsid() -> i32;
}

#[cfg(unix)]
fn libc_setsid() {
    // Safety: setsid() has no failure mode that affects us here. If it
    // fails (e.g. the child is already a process-group leader) we fall
    // back to the `sysinfo` descendant walk in `kill_process_tree`.
    unsafe {
        let _ = setsid();
    }
}

/// Put `cmd`'s child into a brand-new session + process group, so its
/// PID *is* its PGID and `killpg(child_pid, ..)` targets exactly the
/// tree we spawned (shell + every descendant that didn't call `setsid`
/// itself).
///
/// No-op on non-Unix: callers fall back to per-PID kills.
#[cfg(unix)]
pub fn isolate_process_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    // Safety: setsid(2) is always safe to call from a child pre-exec
    // hook; it just creates a new session and process group. It runs
    // between fork and exec, where only async-signal-safe work is
    // allowed — setsid is one of the few syscalls that qualifies.
    unsafe {
        cmd.pre_exec(|| {
            libc_setsid();
            Ok(())
        });
    }
}

#[cfg(not(unix))]
pub fn isolate_process_group(_cmd: &mut Command) {}

/// [`isolate_process_group`] for `tokio::process::Command`. tokio
/// re-exports `CommandExt`, so the same `pre_exec` hook applies.
#[cfg(unix)]
pub fn isolate_process_group_tokio(cmd: &mut tokio::process::Command) {
    // Safety: see `isolate_process_group`.
    unsafe {
        cmd.pre_exec(|| {
            libc_setsid();
            Ok(())
        });
    }
}

#[cfg(not(unix))]
pub fn isolate_process_group_tokio(_cmd: &mut tokio::process::Command) {}

/// Build the standard nini shell invocation: `$SHELL -c <script>`, cwd
/// set, stdin closed, stdout/stderr piped, and — on Unix — in its own
/// process group.
///
/// Used by `BashRunner` (the synchronous hook path).
pub fn shell_command(shell: &str, script: &str, cwd: &std::path::Path) -> Command {
    let mut cmd = Command::new(shell);
    cmd.arg("-c")
        .arg(script)
        .current_dir(cwd)
        // Closed stdin: an interactive command that reads stdin would
        // otherwise block forever holding the tool call open.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate_process_group(&mut cmd);
    cmd
}

/// Async twin of [`shell_command`], for the `BashTool` path — the one the
/// LLM actually calls. Adds `kill_on_drop` so that dropping the awaiting
/// future (abort / cancel) tears the child down instead of leaking it.
///
/// The process-group setup is not duplicated: it goes through the same
/// [`isolate_process_group`], and `procgroup::tests` asserts that *both*
/// constructors produce a group leader. The original bug was precisely
/// a constructor that silently skipped this step.
pub fn shell_command_tokio(
    shell: &str,
    script: &str,
    cwd: &std::path::Path,
) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(shell);
    cmd.arg("-c")
        .arg(script)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    isolate_process_group_tokio(&mut cmd);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read a process's real process-group id (field 5 of
    /// `/proc/<pid>/stat`).
    #[cfg(target_os = "linux")]
    fn real_pgid(pid: u32) -> Option<i32> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The comm field can contain spaces and parentheses, so start
        // the field scan after the final ')'.
        let close = stat.rfind(')')?;
        stat[close + 2..].split_whitespace().nth(2)?.parse().ok()
    }

    /// The whole point of the module: the child's PGID must equal its
    /// own PID, otherwise `killpg(child_pid, ..)` is a no-op.
    ///
    /// Runs against **both** constructors. The v0.8.9 bug existed
    /// because `bash.rs` had its own command-building code that skipped
    /// the isolation step; a test that only covered one constructor
    /// cannot catch that.
    #[test]
    #[cfg(target_os = "linux")]
    fn sync_constructor_isolates_process_group() {
        let mut child = shell_command("/bin/sh", "sleep 5", std::path::Path::new("/tmp"))
            .spawn()
            .expect("spawn /bin/sh");
        let pid = child.id();

        // Give the pre_exec hook a moment to run.
        std::thread::sleep(std::time::Duration::from_millis(150));
        let pgid = real_pgid(pid).expect("read /proc pgid");

        let _ = child.kill();
        let _ = child.wait();

        assert_eq!(
            pgid, pid as i32,
            "child pgid {pgid} != pid {pid} — killpg(child_pid) would be a no-op"
        );
    }

    /// Same invariant, tokio constructor — i.e. the `BashTool` path the
    /// LLM uses, which is the one that was broken.
    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn async_constructor_isolates_process_group() {
        let mut child = shell_command_tokio("/bin/sh", "sleep 5", std::path::Path::new("/tmp"))
            .spawn()
            .expect("spawn /bin/sh");
        let pid = child.id().expect("child pid");

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let pgid = real_pgid(pid).expect("read /proc pgid");

        let _ = child.start_kill();
        let _ = child.wait().await;

        assert_eq!(
            pgid, pid as i32,
            "child pgid {pgid} != pid {pid} — killpg(child_pid) would be a no-op"
        );
    }

    /// The parent must NOT be dragged into the child's group, or a
    /// timeout kill would take nini down with the command.
    #[test]
    #[cfg(target_os = "linux")]
    fn parent_keeps_its_own_process_group() {
        let mut child = shell_command("/bin/sh", "sleep 5", std::path::Path::new("/tmp"))
            .spawn()
            .expect("spawn /bin/sh");
        std::thread::sleep(std::time::Duration::from_millis(150));
        let child_pgid = real_pgid(child.id());
        let _ = child.kill();
        let _ = child.wait();

        let self_stat = std::fs::read_to_string("/proc/self/stat").unwrap();
        let close = self_stat.rfind(')').unwrap();
        let self_pgid: i32 = self_stat[close + 2..]
            .split_whitespace()
            .nth(2)
            .unwrap()
            .parse()
            .unwrap();

        assert_ne!(
            child_pgid,
            Some(self_pgid),
            "child shares nini's process group — a timeout kill would kill nini"
        );
    }
}
